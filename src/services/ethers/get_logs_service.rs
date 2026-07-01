use crate::repositories::ethers::ethers_repository::EthersRepository;
use crate::repositories::sqlite_repository::SqliteRepository;
use crate::utils::bytecode_utils::BytecodeUtils;
use ethers::middleware::Middleware;
use ethers::prelude::{BlockNumber, Provider, Transaction, Ws};
use ethers::types::{Block, BlockId, Bytes};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore, SemaphorePermit};
use tokio::task;
use crate::controllers::dto::ethers_dto::GetLogsDTO;

#[derive(Clone)]
pub struct GetLogsService {
    repository: Arc<RwLock<EthersRepository>>,
    sqlite_repository: Arc<SqliteRepository>,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
struct TransactionWithBytecode {
    #[serde(flatten)]
    transaction: Transaction,
    bytecode: Option<Bytes>,
}

impl GetLogsService {
    pub fn new(
        repository: Arc<RwLock<EthersRepository>>,
        sqlite_repository: Arc<SqliteRepository>,
    ) -> Self {
        GetLogsService {
            repository,
            sqlite_repository,
        }
    }

    fn filter_tx_not_saved(
        &self,
        transactions: Vec<TransactionWithBytecode>,
    ) -> Vec<TransactionWithBytecode> {
        transactions
            .into_iter()
            .filter(|tx| {
                let hash = format!("{:?}", tx.transaction.hash);
                !self.sqlite_repository.deployment_exists(&hash).unwrap_or(false)
            })
            .collect()
    }

    pub async fn exec(&self, user_id: i64, dto: &GetLogsDTO) {
        let semaphore = Arc::new(Semaphore::new(300));

        for block_number in dto.from_block..=dto.to_block {
            let provider = match self.repository.read().await.get_connection(user_id) {
                Some(p) => p,
                None => {
                    eprintln!("[LOGS] Provider não encontrado para user_id: {}", user_id);
                    return;
                }
            };
            let semaphore = Arc::clone(&semaphore);
            let my_clone = self.clone();

            task::spawn(async move {
                let _: SemaphorePermit = match semaphore.acquire().await {
                    Ok(p) => p,
                    Err(_) => return,
                };

                match Self::fetch_and_process_block(provider.clone(), block_number).await {
                    Some(block) => {
                        my_clone
                            .process_block_transactions(block.transactions, provider.clone())
                            .await;
                        eprintln!("[LOGS] Block {} read!", block_number);
                    }
                    None => {
                        eprintln!("[LOGS] Falha ao processar bloco {}", block_number);
                    }
                }
            });
        }
    }

    async fn process_block_transactions(
        &self,
        transactions: Vec<Transaction>,
        provider: Arc<Provider<Ws>>,
    ) {
        let deploy_txs = self.find_erc20_deploys(provider, transactions).await;
        self.filter_and_store(deploy_txs);
    }

    async fn find_erc20_deploys(
        &self,
        provider: Arc<Provider<Ws>>,
        transactions: Vec<Transaction>,
    ) -> Vec<TransactionWithBytecode> {
        let mut results = Vec::new();

        for transaction in transactions {
            let is_deploy_erc20 =
                BytecodeUtils::bytecode_is_deploy_erc20(transaction.input.to_string())
                    && transaction.to.is_none();
            if is_deploy_erc20 {
                let receipt = match provider.get_transaction_receipt(transaction.hash).await {
                    Ok(Some(r)) => r,
                    Ok(None) => continue,
                    Err(e) => {
                        eprintln!("[LOGS] Erro ao obter receipt: {}", e);
                        continue;
                    }
                };

                let contract_addr = match receipt.contract_address {
                    Some(a) => a,
                    None => continue,
                };

                let bytecode = match provider.get_code(contract_addr, None).await {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("[LOGS] Erro ao obter bytecode: {}", e);
                        continue;
                    }
                };

                let update_to = Transaction {
                    to: receipt.contract_address,
                    ..transaction
                };

                results.push(TransactionWithBytecode {
                    transaction: update_to,
                    bytecode: Some(bytecode),
                });
            }
        }

        results
    }

    fn filter_and_store(&self, transactions: Vec<TransactionWithBytecode>) {
        let not_saved = self.filter_tx_not_saved(transactions);

        if not_saved.is_empty() {
            return;
        }

        let pairs: Vec<(String, String)> = not_saved
            .iter()
            .map(|tx| {
                let hash = format!("{:?}", tx.transaction.hash);
                let data = serde_json::to_string(tx).unwrap_or_default();
                (hash, data)
            })
            .collect();

        if let Err(e) = self.sqlite_repository.bulk_insert_deployments(&pairs) {
            eprintln!("[LOGS] Erro ao indexar no SQLite: {}", e);
        }
    }

    async fn fetch_and_process_block(
        provider: Arc<Provider<Ws>>,
        block_number: u64,
    ) -> Option<Block<Transaction>> {
        let block_id = BlockId::Number(BlockNumber::Number(block_number.into()));
        match provider.get_block_with_txs(block_id).await {
            Ok(Some(block)) => Some(block),
            Ok(None) => {
                eprintln!("[LOGS] Bloco {} não encontrado", block_number);
                None
            }
            Err(e) => {
                eprintln!("[LOGS] Erro ao buscar bloco {}: {}", block_number, e);
                None
            }
        }
    }
}
