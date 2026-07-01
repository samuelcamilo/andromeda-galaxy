use crate::utils::bytecode_utils::BytecodeUtils;
use ethers::middleware::Middleware;
use ethers::prelude::{Provider, Transaction, Ws, H160, H256};
use ethers::types::{Address, BlockId, TransactionReceipt, U64};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::{self, JoinHandle};
use crate::services::ethers::find_deploys::find_by_debug_trace::FindByDebugTrace;
use crate::services::ethers::find_deploys::find_by_logs::{FindByLogs};
use crate::services::ethers::find_deploys::find_on_receipt::FindOnReceipt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindDeploysPayload {
    pub contract_address: H160,
    pub from: H160,
    pub input: String,
    pub block_number: Option<U64>,
}

pub struct FindDeploysService {}

impl FindDeploysService {

    pub async fn exec(
        provider: Arc<Provider<Ws>>,
        transactions: Vec<Transaction>,
    ) -> Vec<FindDeploysPayload> {
        // Concurrency cap on RPC fan-out per block. The semaphore prevents us
        // from saturating the WS provider when a block has hundreds of txs.
        let semaphore = Arc::new(Semaphore::new(64));
        let mut handles: Vec<JoinHandle<Option<FindDeploysPayload>>> =
            Vec::with_capacity(transactions.len());

        // Spawn every tx in parallel. We MUST NOT await each handle in the loop:
        // a single block with 200 txs serialized would take 20s+ and we would
        // miss subsequent blocks (block time is ~12s).
        for transaction in transactions {
            let provider = provider.clone();
            let semaphore = Arc::clone(&semaphore);

            let handle = task::spawn(async move {
                let _permit = match semaphore.acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => return None,
                };
                Self::find(provider, transaction).await
            });
            handles.push(handle);
        }

        let mut founds: Vec<FindDeploysPayload> = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(Some(value)) => founds.push(value),
                Ok(None) => {}
                Err(e) => eprintln!("[DEPLOY] Task panicked: {}", e),
            }
        }

        founds
    }

    async fn find(
        provider: Arc<Provider<Ws>>,
        transaction: Transaction,
    ) -> Option<FindDeploysPayload> {
        // let hash = H256::from_str("0xaa37590e70d68bc5859a332361ba91848ea1c30603a8269f162b90587f2e2506").expect("ERR");
        // let receipt = provider.get_transaction_receipt(hash);
        let receipt = provider.get_transaction_receipt(transaction.hash);

        match receipt.await {
            Ok(Some(receipt)) => {
                if let Some(payload) = FindOnReceipt::exec(provider.clone(), receipt.clone()).await {
                    return Some(payload);
                }
                //
                // if let Some(payload) = FindByLogs::exec(provider.clone(), receipt.clone()).await {
                //     return Some(payload);
                // }
                //
                //
                let is_create2 = BytecodeUtils::bytecode_is_create2(transaction.input.to_string());
                if is_create2 {
                    if let Some(payload) = FindByDebugTrace::exec(provider.clone(), receipt.clone()).await {
                        return Some(payload);
                    }
                }
                //


                None
            }
            Err(err) => {
                eprintln!("Erro ao obter receipt: {}", err);
                None
            }
            _ => None,
        }
    }

    pub async fn validate_and_create_payload(
        provider: Arc<Provider<Ws>>,
        contract_address: H160,
        receipt: &TransactionReceipt,
        proxy:Option<H160>
    ) -> Option<FindDeploysPayload> {
        let code_address = proxy.unwrap_or(contract_address);

        if let Ok(bytecode) = provider.get_code(code_address, None).await {

            let is_deploy_erc20 = BytecodeUtils::bytecode_is_deploy_erc20(bytecode.to_string());
            let is_not_erc20 = BytecodeUtils::bytecode_is_not_erc20(bytecode.to_string());

            if is_deploy_erc20 && !is_not_erc20 {
                return Some(FindDeploysPayload {
                    input: bytecode.to_string(),
                    contract_address,
                    from: receipt.from,
                    block_number:receipt.block_number
                });
            }
        }
        None
    }
}
