use crate::controllers::dto::ethers_dto::ListenDeployErc20ContractsDTO;
use crate::http_client::HttpClient;
use crate::repositories::ethers::ethers_repository::EthersRepository;
use crate::services::ethers::find_deploys::find_deploys_service::{FindDeploysPayload, FindDeploysService};
use crate::services::telegram_service::TelegramService;
use ethers::prelude::{Block, BlockNumber, Provider, Ws, H256};
use ethers::providers::Middleware;
use ethers::types::{BlockId, TransactionReceipt};
use futures::FutureExt;
use serde::Serialize;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio::sync::RwLock;

pub struct ListenDeployErc20ContractsService {
    repository: Arc<RwLock<EthersRepository>>,
    telegram_service: Arc<TelegramService>,
}

#[derive(Serialize)]
struct Payload {
    transaction: TransactionReceipt,
    input: String,
}

impl ListenDeployErc20ContractsService {
    pub fn new(
        repository: Arc<RwLock<EthersRepository>>,
        telegram_service: Arc<TelegramService>,
    ) -> Self {
        ListenDeployErc20ContractsService {
            repository,
            telegram_service,
        }
    }

    pub async fn exec(&self, user_id: i64, dto: &ListenDeployErc20ContractsDTO) {
        let mut repo = self.repository.write().await;
        let block_listener = repo.get_block_listener(user_id);
        let shared_provider = repo.get_shared_provider(user_id);
        drop(repo);

        if let (Some(receiver), Some(shared)) = (block_listener, shared_provider) {
            let repository = self.repository.clone();
            let telegram = self.telegram_service.clone();
            tokio::spawn(Self::spawn_process_task(
                repository,
                user_id,
                dto.webhook.clone(),
                receiver,
                telegram,
                shared,
            ));
        } else {
            eprintln!("Nenhum listener configurado para o user_id: {}", user_id);
        }
    }

    async fn process_block(
        provider: &Arc<Provider<Ws>>,
        block: Block<H256>,
    ) -> Vec<FindDeploysPayload> {
        let block_number = match block.number {
            Some(n) => n,
            None => {
                eprintln!("[BLOCK] Bloco sem número, pulando");
                return Vec::new();
            }
        };

        let block_id = BlockId::Number(BlockNumber::Number(block_number));
        let block_data = match provider.get_block_with_txs(block_id).await {
            Ok(Some(b)) => b,
            Ok(None) => {
                eprintln!("[BLOCK] Bloco {} não encontrado", block_number);
                return Vec::new();
            }
            Err(e) => {
                eprintln!("[BLOCK] Erro ao buscar bloco {}: {}", block_number, e);
                return Vec::new();
            }
        };

        FindDeploysService::exec(provider.clone(), block_data.transactions).await
    }

    async fn send_transactions(
        webhook: String,
        transactions: &[FindDeploysPayload],
    ) -> Result<(), reqwest::Error> {
        let client = HttpClient::new();

        client
            .get_client()
            .post(&webhook)
            .json(transactions)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    fn spawn_process_task(
        _repository: Arc<RwLock<EthersRepository>>,
        _user_id: i64,
        webhook: String,
        mut receiver: Receiver<Block<H256>>,
        telegram: Arc<TelegramService>,
        shared_provider: Arc<tokio::sync::RwLock<Arc<Provider<Ws>>>>,
    ) -> impl std::future::Future<Output = ()> {
        async move {
            // Limita quantos blocos s\u00e3o processados em paralelo. Sem isso, um bloco
            // lento poderia segurar todos os pr\u00f3ximos atr\u00e1s dele e fazer o bot
            // perder deploys quando o RPC engasga.
            let block_concurrency = Arc::new(tokio::sync::Semaphore::new(8));

            while let Some(block) = receiver.recv().await {
                let block_num = block.number.map(|n| n.as_u64()).unwrap_or(0);
                let provider = shared_provider.read().await.clone();
                let webhook = webhook.clone();
                let telegram = telegram.clone();
                let block_concurrency = block_concurrency.clone();
                let permit = match block_concurrency.acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => return,
                };

                tokio::spawn(async move {
                    let _permit = permit;

                    let result = tokio::time::timeout(
                        Duration::from_secs(60),
                        AssertUnwindSafe(Self::process_block(&provider, block)).catch_unwind(),
                    )
                    .await;

                    let transactions = match result {
                        Ok(Ok(txs)) => txs,
                        Ok(Err(_)) => {
                            eprintln!(
                                "[LISTENER] Panic capturado no bloco {}, continuando...",
                                block_num
                            );
                            return;
                        }
                        Err(_) => {
                            eprintln!(
                                "[LISTENER] Timeout processando bloco {}, continuando...",
                                block_num
                            );
                            return;
                        }
                    };

                    if !transactions.is_empty() {
                        if let Err(err) =
                            Self::send_transactions(webhook, &transactions).await
                        {
                            eprintln!("[WEBHOOK] Erro ao enviar webhook: {}", err);
                        }

                        for payload in &transactions {
                            telegram.notify(provider.clone(), payload.clone());
                        }
                    }
                });
            }

            eprintln!("[LISTENER] Receiver encerrado para user_id: {}", _user_id);
        }
    }
}
