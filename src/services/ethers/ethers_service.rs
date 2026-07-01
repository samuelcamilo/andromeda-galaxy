use crate::controllers::dto::ethers_dto::{ApplyRpcDTO, GetCodeDTO, GetTransactionCountDTO};
use crate::repositories::ethers::ethers_repository::EthersRepository;
use ethers::addressbook::Address;
use ethers::middleware::Middleware;
use ethers::prelude::{BlockId, BlockNumber, Bytes, NameOrAddress, Provider, Transaction, Ws, H256, U256};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct EthersService {
    repository: Arc<RwLock<EthersRepository>>,
}

impl EthersService {
    pub fn new(repository: Arc<RwLock<EthersRepository>>) -> Self {
        EthersService { repository }
    }

    pub async fn get_transaction_count(&self, user_id: i64, dto: &GetTransactionCountDTO) -> Result<U256, String> {
        let block_number = dto.block_number.map(|number| BlockId::Number(BlockNumber::Number(number.into())));
        let addr = Address::from_str(&dto.address).map_err(|e| format!("Endereço inválido: {}", e))?;
        let provider = self.get_provider(user_id).await?;
        provider
            .get_transaction_count(NameOrAddress::Address(addr), block_number)
            .await
            .map_err(|e| format!("Erro ao obter nonce: {}", e))
    }

    pub async fn get_balance(&self, user_id: i64, dto: &GetTransactionCountDTO) -> Result<U256, String> {
        let addr = Address::from_str(&dto.address).map_err(|e| format!("Endereço inválido: {}", e))?;
        let provider = self.get_provider(user_id).await?;
        let block_number = dto.block_number.map(|number| BlockId::Number(BlockNumber::Number(number.into())));
        provider
            .get_balance(NameOrAddress::Address(addr), block_number)
            .await
            .map_err(|e| format!("Erro ao obter balance: {}", e))
    }

    pub async fn get_code(&self, user_id: i64, dto: &GetCodeDTO) -> Result<Bytes, String> {
        let addr = Address::from_str(&dto.address).map_err(|e| format!("Endereço inválido: {}", e))?;
        let provider = self.get_provider(user_id).await?;
        provider
            .get_code(NameOrAddress::Address(addr), None)
            .await
            .map_err(|e| format!("Erro ao obter code: {}", e))
    }

    async fn get_provider(&self, user_id: i64) -> Result<Arc<Provider<Ws>>, String> {
        let lock = self.repository.read().await;
        lock.get_connection(user_id)
            .ok_or_else(|| format!("Provider não encontrado para user_id: {}", user_id))
    }

    pub async fn get_transaction(&self, user_id: i64, transaction_hash: H256) -> Result<Option<Transaction>, String> {
        let provider = self.get_provider(user_id).await?;
        provider
            .get_transaction(transaction_hash)
            .await
            .map_err(|e| format!("Erro ao obter transaction: {}", e))
    }

    pub async fn apply_rpc(&self, user_id: i64, dto: &ApplyRpcDTO) -> Result<(), String> {
        if self.repository.read().await.get_connection(user_id).is_some() {
            return Ok(());
        }

        let provider = Provider::<Ws>::connect(dto.endpoint.clone())
            .await
            .map_err(|e| format!("Falha ao conectar WebSocket: {}", e))?;

        let mut repo = self.repository.write().await;
        repo.set_endpoint(user_id, dto.endpoint.clone());
        repo.apply_connection(user_id, provider.clone());
        repo.apply_block_listener(user_id, provider.clone()).await;

        Ok(())
    }
}