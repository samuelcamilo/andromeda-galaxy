use crate::controllers::dto::ethers_dto::ListenContractEventsDTO;
use crate::http_client::HttpClient;
use crate::repositories::ethers::ethers_repository::EthersRepository;
use ethers::abi::Address;
use ethers::middleware::Middleware;
use ethers::prelude::{Filter, Provider, StreamExt, Transaction, Ws, H256};
use ethers::types::{Bytes, H160};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ListenContractEventsService {
    repository: Arc<RwLock<EthersRepository>>,
}

#[derive(Serialize, Debug, Clone)]
struct PayloadContractEvent {
    address: Address,
    transaction: Transaction,
    user_id: i64,
    input: Bytes,
}

impl ListenContractEventsService {
    pub fn new(repository: Arc<RwLock<EthersRepository>>) -> Self {
        ListenContractEventsService { repository }
    }

    pub async fn exec(&self, user_id: i64, dto: &ListenContractEventsDTO) {
        let repository = self.repository.clone();
        tokio::spawn(Self::spawn_process_task(
            repository,
            user_id,
            dto.address.clone(),
            dto.event_signature.clone(),
            dto.webhook.clone(),
        ));
    }

    async fn get_provider(
        repository: Arc<RwLock<EthersRepository>>,
        user_id: i64,
    ) -> Option<Arc<Provider<Ws>>> {
        repository.read().await.get_connection(user_id)
    }

    fn create_event_filter(contract_address: Address, event_signature: &str) -> Filter {
        let event_signature_hash = H256::from_slice(&ethers::utils::keccak256(event_signature));
        Filter::new()
            .address(contract_address)
            .topic0(event_signature_hash)
    }

    async fn send_transaction(
        webhook: String,
        transaction: PayloadContractEvent,
    ) {
        let client = HttpClient::new();

        if let Err(e) = client
            .get_client()
            .post(&webhook)
            .json(&transaction)
            .send()
            .await
        {
            eprintln!("[EVENT] Erro ao enviar webhook: {}", e);
        }
    }

    async fn process_event(
        user_id: i64,
        provider: Arc<Provider<Ws>>,
        hash: H256,
        webhook: String,
        address: H160,
        input: Bytes,
    ) {
        match provider.get_transaction(hash).await {
            Ok(Some(value)) => {
                let payload = PayloadContractEvent {
                    address,
                    transaction: value,
                    user_id,
                    input,
                };
                Self::send_transaction(webhook, payload).await;
            }
            Ok(None) => {
                eprintln!("[EVENT] Transação não encontrada: {:?}", hash);
            }
            Err(e) => {
                eprintln!("[EVENT] Erro ao obter transação: {}", e);
            }
        }
    }

    fn spawn_process_task(
        repository: Arc<RwLock<EthersRepository>>,
        user_id: i64,
        contract_address: String,
        event_signature: String,
        webhook: String,
    ) -> impl std::future::Future<Output = ()> {
        async move {
            let provider = match Self::get_provider(repository.clone(), user_id).await {
                Some(p) => p,
                None => {
                    eprintln!("[EVENT] Provider não encontrado para user_id: {}", user_id);
                    return;
                }
            };

            let contract_address: Address = match contract_address.parse() {
                Ok(addr) => addr,
                Err(e) => {
                    eprintln!("[EVENT] Endereço inválido: {}", e);
                    return;
                }
            };

            let filter = Self::create_event_filter(contract_address, &event_signature);

            let mut stream = match provider.subscribe_logs(&filter).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[EVENT] Erro ao criar stream de logs: {}", e);
                    return;
                }
            };

            while let Some(log_result) = stream.next().await {
                let hash = match log_result.transaction_hash {
                    Some(h) => h,
                    None => continue,
                };
                let input = log_result.data;

                Self::process_event(
                    user_id,
                    provider.clone(),
                    hash,
                    webhook.clone(),
                    contract_address,
                    input,
                )
                .await;
            }

            eprintln!("[EVENT] Stream de eventos encerrou para user_id: {}", user_id);
        }
    }
}
