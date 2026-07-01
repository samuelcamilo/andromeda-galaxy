use crate::controllers::dto::cron_job_dto::RecallSomebodyWebhookDTO;
use crate::http_client::HttpClient;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{self, Duration};

pub struct CronJobService {
    interruptor: Arc<RwLock<HashMap<String, bool>>>,
}

impl CronJobService {
    pub fn new() -> Self {
        CronJobService {
            interruptor: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn recall_samebody_webhook(&self, dto: &RecallSomebodyWebhookDTO) {
        let identifier = dto.identifier.clone();

        if self.check_and_update_state(&identifier).await {
            return;
        }

        self.spawn_webhook_task(dto).await;
    }

    async fn check_and_update_state(&self, identifier: &str) -> bool {
        let current_state = self.get_state(identifier).await;

        if current_state {
            self.set_state(identifier, false).await;
            true
        } else {
            self.set_state(identifier, true).await;
            false
        }
    }

    async fn get_state(&self, identifier: &str) -> bool {
        let interruptor = self.interruptor.read().await;
        *interruptor.get(identifier).unwrap_or(&false)
    }

    async fn set_state(&self, identifier: &str, value: bool) {
        let mut interruptor = self.interruptor.write().await;
        interruptor.insert(identifier.to_string(), value);
    }

    async fn spawn_webhook_task(&self, dto: &RecallSomebodyWebhookDTO) {
        let webhook_clone = dto.webhook.clone();
        let body_clone = dto.body.clone();
        let timer = dto.timer;
        let interruptor = Arc::clone(&self.interruptor);
        let identifier = dto.identifier.clone();

        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(timer));

            let client = HttpClient::new();

            loop {
                interval.tick().await;

                if let Err(err) = client
                    .get_client()
                    .post(&webhook_clone)
                    .json(&body_clone)
                    .send()
                    .await
                    .and_then(|response| response.error_for_status())
                {
                    println!("Erro ao enviar webhook: {:#?}", err); // Mostra o erro no console
                    let read_lock = &interruptor.read().await;
                    read_lock.get(&identifier).unwrap_or(&false);
                    break;
                }

                let state = {
                    let read_lock = &interruptor.read().await;
                    *read_lock.get(&identifier).unwrap_or(&false)
                };

                if !state {
                    break;
                }
            }
        });
    }
}