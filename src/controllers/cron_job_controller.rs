use std::collections::HashMap;
use std::sync::Arc;
use actix_web::{web, HttpResponse, Responder, Route};
use crate::controllers::dto::cron_job_dto::RecallSomebodyWebhookDTO;
use crate::services::cron_job_service::CronJobService;

pub struct CronJobController;

impl CronJobController {

    pub fn new () -> Self {
        CronJobController
    }

    pub async fn recall_somebody_webhook_ctrl(
        service: web::Data<Arc<CronJobService>>,
        data: web::Json<RecallSomebodyWebhookDTO>
    ) -> impl Responder {
        service.recall_samebody_webhook(&data).await;
        HttpResponse::Ok()
    }

    pub fn routes(self) -> HashMap<String, Route> {
        let mut routes = HashMap::new();

        routes.insert(String::from("cronjob/recall_samebody"), web::post().to(Self::recall_somebody_webhook_ctrl));

        routes
    }
}