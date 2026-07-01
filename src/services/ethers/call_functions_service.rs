use ethers::abi::{Abi, Token};
use ethers::contract::{Contract, ContractInstance};
use ethers::prelude::{Provider, Ws};
use ethers::types::Address;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::controllers::dto::ethers_dto::CallFunctionsDTO;
use crate::repositories::ethers::ethers_repository::EthersRepository;
use crate::utils::ethers_utils::EthersUtils;

pub struct CallFunctionsService {
    repository: Arc<RwLock<EthersRepository>>,
}

impl CallFunctionsService {
    pub fn new(repository: Arc<RwLock<EthersRepository>>) -> Self {
        CallFunctionsService { repository }
    }

    async fn get_provider(&self, user_id: i64) -> Option<Arc<Provider<Ws>>> {
        let lock = self.repository.read().await;
        lock.get_connection(user_id)
    }

    fn create_contract(
        provider: Arc<Provider<Ws>>,
        contract_address: String,
        abi: String,
    ) -> Option<ContractInstance<Arc<Provider<Ws>>, Provider<Ws>>> {
        let parsed_abi: Abi = serde_json::from_str(&abi).ok()?;
        let token_address: Address = contract_address.parse().ok()?;
        Some(Contract::new(token_address, parsed_abi, provider))
    }

    async fn get_call_response(
        contract: ContractInstance<Arc<Provider<Ws>>, Provider<Ws>>,
        function_name: &str,
    ) -> Option<Token> {
        match contract.method(&function_name, ()) {
            Ok(method) => method.call().await.ok(),
            Err(_) => None,
        }
    }

    pub async fn exec(
        &self,
        user_id: i64,
        dto: &CallFunctionsDTO,
    ) -> HashMap<String, Value> {
        let mut functions_response: HashMap<String, Value> = HashMap::new();

        let provider = match self.get_provider(user_id).await {
            Some(p) => p,
            None => return functions_response,
        };

        let contract = match Self::create_contract(provider.clone(), dto.address.clone(), dto.abi.clone()) {
            Some(c) => c,
            None => return functions_response,
        };

        for function_name in dto.functions_name.clone() {
            if let Some(token) = Self::get_call_response(contract.clone(), &function_name).await {
                let token_to_json = EthersUtils::token_to_json(token);
                functions_response.insert(function_name, token_to_json);
            }
        }

        functions_response
    }
}