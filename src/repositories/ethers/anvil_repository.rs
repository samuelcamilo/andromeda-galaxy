use ethers::middleware::Middleware;
use ethers::prelude::{Bytes, PendingTransaction, Provider, ProviderError, TransactionRequest};
use ethers::providers::Http;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, BlockId, U256};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct ManagedAnvilInstance {
    child: Child,
}

impl ManagedAnvilInstance {
    pub async fn spawn_forking_provider(
        endpoint: &str,
        block_number: Option<u64>,
    ) -> Result<(Provider<Http>, Self), String> {
        let port = Self::pick_unused_port()?;
        let mut cmd = Command::new(Self::anvil_path());
        cmd.arg("-p")
            .arg(port.to_string())
            .arg("-f")
            .arg(endpoint)
            .arg("--auto-impersonate")
            .arg("--no-mining")
            .arg("--gas-price=0")
            .arg("--base-fee=0")
            .arg("--gas-limit=30000000")
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if let Some(block) = block_number {
            cmd.arg("--fork-block-number").arg(block.to_string());
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn anvil: {}", e))?;
        let mut instance = ManagedAnvilInstance { child };
        let endpoint = format!("http://127.0.0.1:{}", port);
        let provider = Provider::<Http>::try_from(endpoint.as_str())
            .map_err(|e| format!("failed to create anvil provider: {}", e))?;

        let started_at = Instant::now();
        while started_at.elapsed() < Duration::from_secs(10) {
            if let Ok(Some(status)) = instance.child.try_wait() {
                return Err(format!("anvil exited during startup: {}", status));
            }

            match tokio::time::timeout(Duration::from_secs(1), provider.get_block_number()).await {
                Ok(Ok(_)) => return Ok((provider, instance)),
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }

        Err("timed out waiting for anvil to start".to_string())
    }

    fn pick_unused_port() -> Result<u16, String> {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|addr| addr.port())
            .map_err(|e| format!("failed to pick anvil port: {}", e))
    }

    fn anvil_path() -> &'static str {
        if Path::new("/usr/local/bin/anvil").exists() {
            "/usr/local/bin/anvil"
        } else if Path::new("/root/.foundry/bin/anvil").exists() {
            "/root/.foundry/bin/anvil"
        } else {
            "anvil"
        }
    }
}

impl Drop for ManagedAnvilInstance {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            Err(_) => {}
        }
    }
}

pub struct AnvilRepository {
    forked_connections: HashMap<String, Arc<Provider<Http>>>,
    forked_anvil: HashMap<String, ManagedAnvilInstance>,
    block_timestamps: HashMap<String, u64>, // Novo campo
}

impl AnvilRepository {
    pub fn new() -> Self {
        AnvilRepository {
            forked_connections: HashMap::new(),
            forked_anvil: HashMap::new(),
            block_timestamps: Default::default(),
        }
    }

    pub fn set_block_timestamp(&mut self, identifier: String, timestamp: u64) {
        self.block_timestamps.insert(identifier, timestamp);
    }

    pub fn get_block_timestamp(&self, identifier: &str) -> Option<u64> {
        self.block_timestamps.get(identifier).copied()
    }
    
    pub fn apply_forking_provider(
        &mut self,
        identifier: String,
        provider: Provider<Http>,
        anvil: ManagedAnvilInstance,
    ) {
        let provider = Arc::new(provider);
        self.forked_anvil.insert(identifier.clone(), anvil);
        self.forked_connections.insert(identifier, provider);
    }
    pub fn get_fork_connection(&self, identifier: String) -> Option<Arc<Provider<Http>>> {
        self.forked_connections.get(&identifier).cloned()
    }

    pub async fn impersonate_account(&self, identifier: String, address:Address) {
        let provider = self.forked_connections.get(&identifier).unwrap();

        provider
            .request::<_, serde_json::Value>(
                "anvil_impersonateAccount",
                json!([address]),
            )
            .await
            .expect("Failed to impersonate account");
    }

    pub async fn call_transaction(&self, identifier: String, transaction:TransactionRequest,block_number:Option<BlockId>) -> Result<Bytes, Box<ProviderError>> {
        let provider = self.forked_connections.get(&identifier).unwrap();
        let typed_tx: TypedTransaction = transaction.into();

        let call_tx = match provider.call(&typed_tx, block_number).await {
            Ok(response) => response,
            Err(e) => {
                eprintln!("Erro ao enviar transação para o usuário {}: {}", identifier, e);
                return Err(Box::new(e));
            }
        };

        Ok(call_tx)
    }

    pub async fn send_transaction(&self, identifier: String, transaction:TransactionRequest,block_number:Option<BlockId>) -> Result<PendingTransaction<Http>, Box<ProviderError>> {
        let provider = self.forked_connections.get(&identifier).unwrap();

        let pending_tx = match provider.send_transaction(transaction, block_number).await {
            Ok(pending_tx) => pending_tx,
            Err(e) => {
                eprintln!("Erro ao enviar transação para o usuário {}: {}", identifier, e);
                return Err(Box::new(e));
            }
        };

        Ok(pending_tx)
    }

    pub async fn mine(&self, identifier: String) {
        let provider = self.forked_connections.get(&identifier).unwrap();
        let timestamp = self.block_timestamps.get(&identifier).copied().expect("Timestamp não encontrado");

        provider
            .request::<[serde_json::Value; 1], serde_json::Value>(
                "evm_setNextBlockTimestamp",
                [json!(timestamp)],
            )
            .await
            .expect("Failed to set next block timestamp");

        provider
            .request::<_, serde_json::Value>(
                "evm_mine",
                json!([]),
            )
            .await
            .expect("Failed to mine account");
    }

    pub async fn get_balance(&self, identifier: String, address:Address) -> ethers::types::U256 {
        let provider = self.forked_connections.get(&identifier).unwrap();
        let balance = provider.get_balance(address, None).await.unwrap();
        balance
    }

    pub async fn get_transaction_count(&self, identifier: String, address:Address) -> U256 {
        let provider = self.forked_connections.get(&identifier).unwrap();
        let count = provider.get_transaction_count(address, None).await.unwrap();
        count
    }

    pub async fn set_balance(&self, identifier: String, address:Address, balance:ethers::types::U256) {
        let provider = self.forked_connections.get(&identifier).unwrap();

        provider
            .request::<_, serde_json::Value>(
                "anvil_setBalance",
                json!([address,balance]),
            )
            .await
            .expect("Failed to set balance account");
    }

    // Método para remover uma instância de Anvil
    pub fn remove_anvil_instance(&mut self, identifier: &str) {
        let forks_before = self.forked_anvil.len();
        self.forked_connections.remove(identifier);
        self.forked_anvil.remove(identifier);
        eprintln!(
            "[ANVIL] Fork {} removido (forks ativos: {} -> {})",
            identifier,
            forks_before,
            self.forked_anvil.len()
        );
    }


}
