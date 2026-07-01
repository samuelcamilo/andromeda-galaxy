use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc};
use ethers::abi::{decode, ParamType, Token};
use ethers::middleware::Middleware;
use tokio::sync::RwLock;
use ethers::prelude::{Provider, Ws};
use ethers::types::{Bytes, TransactionRequest, H160};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::utils::hex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::controllers::dto::ethers_dto::CallSelectorsDTO;
use crate::repositories::ethers::ethers_repository::EthersRepository;
use crate::utils::ethers_utils::EthersUtils;

#[derive(Serialize, Deserialize, Debug,Clone)]
pub struct SelectorResponse {
    bytes:Bytes,
    value: Option<Value>
}

pub struct CallSelectorsService {
    repository:Arc<RwLock<EthersRepository>>
}

impl CallSelectorsService {
    pub fn new(repository:Arc<RwLock<EthersRepository>>) -> Self {
        CallSelectorsService {repository }
    }

    fn make_transaction_request(address: String, selector_id: String) -> Option<TransactionRequest> {
        let to = H160::from_str(&address).ok()?;
        let data = hex::decode(selector_id).ok()?;
        Some(TransactionRequest::new().to(to).data(Bytes::from(data)))
    }

    async fn call_contract(
        provider: Arc<Provider<Ws>>,
        typed_tx: TypedTransaction,
    ) -> Option<Bytes> {
        match provider.call(&typed_tx, None).await {
            Ok(bytes) => Some(bytes),
            Err(_) => None
        }
    }

    async fn process_selector(
        selector_id: String,
        address: String,
        provider: Arc<Provider<Ws>>,
        responses: Arc<RwLock<HashMap<String, SelectorResponse>>>,
    ) {
        let tx_request = match Self::make_transaction_request(address.clone(), selector_id.clone()) {
            Some(r) => r,
            None => return,
        };
        let typed_tx = TypedTransaction::Legacy(tx_request);

        let result = Self::call_contract(provider, typed_tx).await.unwrap_or_else(|| Bytes::new());

        let mut response = SelectorResponse {
            bytes: result.clone(),
            value: None,
        };

        if !result.is_empty() {
            if let Some(decoded) = Self::decode_result(result.clone()) {
                let value: Value = EthersUtils::token_to_json(decoded);
                response.value = Some(value);
            }
        }

        responses.write().await.insert(selector_id, response);
    }

    fn decode_result(result: Bytes) -> Option<Token> {
        if let Ok(decoded) = decode(&[ParamType::String], &result) {
            return decoded.into_iter().next();
        }
        if let Ok(decoded) = decode(&[ParamType::Uint(256)], &result) {
            return decoded.into_iter().next();
        }
        None
    }

    pub async fn exec(
        &self,
        user_id: i64,
        dto:&CallSelectorsDTO
    ) -> HashMap<String, SelectorResponse> {
        let responses = Arc::new(RwLock::new(HashMap::new()));
        let mut handles = Vec::new();

        for selector_id in dto.selectors_id.clone() {
            let provider = self.get_provider(user_id).await;
            let address = dto.address.clone();
            let responses = responses.clone();

            let handle = tokio::spawn(async move {
                Self::process_selector(selector_id, address, provider, responses).await;
            });

            handles.push(handle);
        }

        for handle in handles {
            if let Err(e) = handle.await {
                eprintln!("[SELECTORS] Task panicked: {}", e);
            }
        }

        let responses = responses.read().await;
        responses.clone()
    }
    async fn get_provider(&self, user_id: i64) -> Arc<Provider<Ws>> {
        let lock = self.repository.read().await;
        lock.get_connection(user_id)
            .unwrap_or_else(|| panic!("Provider não encontrado para user_id: {}", user_id))
    }

}
