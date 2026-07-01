use ethers::middleware::Middleware;
use ethers::providers::{Provider, Ws};
use ethers::types::{Block, H256};
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::mpsc::Receiver;

pub struct EthersRepository {
    connections: HashMap<i64, Arc<Provider<Ws>>>,
    block_listeners: HashMap<i64, Receiver<Block<H256>>>,
    endpoints: HashMap<i64, String>,
    shared_provider: HashMap<i64, Arc<tokio::sync::RwLock<Arc<Provider<Ws>>>>>,
}

impl EthersRepository {
    pub fn new() -> Self {
        EthersRepository {
            connections: HashMap::new(),
            block_listeners: HashMap::new(),
            endpoints: HashMap::new(),
            shared_provider: HashMap::new(),
        }
    }

    pub fn get_block_listener(&mut self, user_id: i64) -> Option<Receiver<Block<H256>>> {
        self.block_listeners.remove(&user_id)
    }

    pub fn apply_connection(&mut self, user_id: i64, provider: Provider<Ws>) {
        let provider = Arc::new(provider);
        let shared = Arc::new(tokio::sync::RwLock::new(provider.clone()));
        self.connections.insert(user_id, provider);
        self.shared_provider.insert(user_id, shared);
    }

    pub fn set_endpoint(&mut self, user_id: i64, endpoint: String) {
        self.endpoints.insert(user_id, endpoint);
    }

    pub fn get_endpoint(&self, user_id: i64) -> Option<String> {
        self.endpoints.get(&user_id).cloned()
    }

    pub fn get_shared_provider(&self, user_id: i64) -> Option<Arc<tokio::sync::RwLock<Arc<Provider<Ws>>>>> {
        self.shared_provider.get(&user_id).cloned()
    }

    pub async fn apply_block_listener(&mut self, user_id: i64, provider: Provider<Ws>) {
        // Buffer grande o bastante para absorver picos / lag de processamento sem
        // bloquear a subscription WS e sem perder cabeças de bloco.
        let (tx, rx) = tokio::sync::mpsc::channel(1024);

        let endpoint = self.endpoints.get(&user_id).cloned();
        let shared = self.shared_provider.get(&user_id).cloned();

        spawn(async move {
            Self::resilient_block_subscription(provider, tx, endpoint, shared).await;
        });

        self.block_listeners.insert(user_id, rx);
    }

    async fn resilient_block_subscription(
        initial_provider: Provider<Ws>,
        tx: tokio::sync::mpsc::Sender<Block<H256>>,
        endpoint: Option<String>,
        shared_provider: Option<Arc<tokio::sync::RwLock<Arc<Provider<Ws>>>>>,
    ) {
        let mut current_provider = initial_provider;
        let mut retry_delay = std::time::Duration::from_secs(1);
        let max_delay = std::time::Duration::from_secs(60);

        loop {
            match current_provider.subscribe_blocks().await {
                Ok(stream) => {
                    retry_delay = std::time::Duration::from_secs(1);
                    eprintln!("[WS] Block subscription ativa");

                    let mut stream = stream;
                    while let Some(block) = stream.next().await {
                        if tx.send(block).await.is_err() {
                            eprintln!("[WS] Receiver dropado, encerrando subscription");
                            return;
                        }
                    }

                    eprintln!("[WS] Stream de blocos encerrou, tentando reconectar...");
                }
                Err(e) => {
                    eprintln!("[WS] Erro ao criar subscription: {}", e);
                }
            }

            if let Some(ref ep) = endpoint {
                eprintln!("[WS] Reconectando em {:?}...", retry_delay);
                tokio::time::sleep(retry_delay).await;

                match Provider::<Ws>::connect(ep.clone()).await {
                    Ok(new_provider) => {
                        eprintln!("[WS] Reconectado com sucesso");
                        current_provider = new_provider;
                        // Update shared provider so listeners use the new connection
                        if let Some(ref shared) = shared_provider {
                            let new_arc = Arc::new(current_provider.clone());
                            *shared.write().await = new_arc;
                        }
                        retry_delay = std::time::Duration::from_secs(1);
                    }
                    Err(e) => {
                        eprintln!("[WS] Falha ao reconectar: {}", e);
                        retry_delay = std::cmp::min(retry_delay * 2, max_delay);
                    }
                }
            } else {
                eprintln!("[WS] Sem endpoint para reconexão, encerrando");
                return;
            }
        }
    }

    pub fn get_connection(&self, user_id: i64) -> Option<Arc<Provider<Ws>>> {
        self.connections.get(&user_id).cloned()
    }
}
