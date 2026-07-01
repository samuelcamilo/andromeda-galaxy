use crate::controllers::dto::ethers_dto::{AnvilGetTransactionCount, ApplyForkDTO, RemoveDTO, SetBalanceDTO, SimulateTxDTO};
use crate::repositories::ethers::anvil_repository::{AnvilRepository, ManagedAnvilInstance};
use actix_web::web::Json;
use ethers::prelude::{Bytes, ProviderError, TransactionReceipt};
use ethers::providers::Middleware;
use ethers::types::{BlockId, BlockNumber, U256};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct AnvilService {
    repository: Arc<RwLock<AnvilRepository>>,
}

impl AnvilService {
    pub fn new(repository: Arc<RwLock<AnvilRepository>>) -> Self {
        AnvilService { repository }
    }

    pub async fn apply_fork(&self,dto: &ApplyForkDTO) {
        let (provider, anvil) = match ManagedAnvilInstance::spawn_forking_provider(
            &dto.endpoint,
            dto.block_number,
        )
        .await
        {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("[ANVIL] falha criando fork manual {}: {}", dto.identifier, e);
                return;
            }
        };
        let block_number = dto.block_number.unwrap_or(0); // ou trate o `None` como preferir
        let block_id = BlockId::Number(BlockNumber::Number(block_number.into()));

        let timestamp = provider
            .get_block(block_id)
            .await
            .ok()
            .flatten()
            .map(|block| block.timestamp.as_u64())
            .unwrap_or(0);

        // Armazena o timestamp
        self.repository.write().await.set_block_timestamp(dto.identifier.clone(), timestamp);

        self.repository
            .write()
            .await
            .apply_forking_provider(dto.identifier.clone(), provider, anvil);
    }

    pub async fn remove(&self,dto:&RemoveDTO) {
        self.repository.write().await.remove_anvil_instance(dto.identifier.as_str());
    }

    pub async fn set_balance(&self, dto: &Json<SetBalanceDTO>) -> Result<(), ProviderError> {
        // Acquire a write lock on the repository
        let lock = self.repository.write().await;

        // Parse the balance in Ether
        let balance = ethers::utils::parse_ether(9999).expect("Failed to parse Ether");

        // Call the `set_balance` method on the repository
        lock.set_balance(dto.identifier.clone(), dto.address.parse().unwrap(), balance).await;

        // Return success
        Ok(())
    }

    pub async fn get_transaction_count(&self, dto: &Json<AnvilGetTransactionCount>) -> Result<U256, Box<ProviderError>> {
        let lock = self.repository.read().await;
        let count = lock.get_transaction_count(dto.identifier.clone(), dto.address.clone().parse().unwrap()).await;
        Ok(count)
    }

    pub async fn simulate_tx(
        &self,
        dto: &Json<SimulateTxDTO>,
    ) -> Result<Option<TransactionReceipt>,ProviderError> {
        let lock = self.repository.write().await;
        let balance = ethers::utils::parse_ether(9999).expect("ERR");

        let block_number = dto.block_number.map(|number| BlockId::Number(BlockNumber::Number(number.into())));
        lock.set_balance(dto.identifier.clone(), dto.transaction.from.expect("ERR"), balance).await;

        let pending_tx = lock.send_transaction(dto.identifier.clone(), dto.transaction.clone(),block_number);

        let pending_tx = match pending_tx.await {
            Ok(tx) => {
                lock.mine(dto.identifier.clone()).await;
                match tx.await {
                    Ok(mined) => {
                        Ok(mined)
                    },
                    Err(e) => Err(e)
                }
            },
            Err(e) => Err(*e)
        };
        pending_tx
    }

    pub async fn call_transaction(&self, dto: &Json<SimulateTxDTO>) -> Result<Bytes, Box<ProviderError>> {
        let lock = self.repository.write().await;
        let block_number = dto.block_number.map(|number| BlockId::Number(BlockNumber::Number(number.into())));

        lock.call_transaction(dto.identifier.clone(), dto.transaction.clone(),block_number)
            .await
    }

}
