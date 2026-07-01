use crate::controllers::dto::telegram_dto::{ConfigureTelegramDTO, TestTelegramDTO};
use crate::services::telegram_service::TelegramService;
use crate::services::telegram_commands::TelegramCommands;
use actix_web::{web, HttpResponse, Responder, Route};
use std::collections::HashMap;
use std::sync::Arc;

pub struct TelegramController;

impl TelegramController {
    pub fn new() -> Self {
        TelegramController
    }

    pub async fn configure_ctrl(
        service: web::Data<Arc<TelegramService>>,
        commands: web::Data<Arc<TelegramCommands>>,
        data: web::Json<ConfigureTelegramDTO>,
    ) -> impl Responder {
        service
            .configure(
                data.bot_token.clone(),
                data.chat_id.clone(),
                data.etherscan_api_key.clone(),
                data.bot_username.clone(),
            )
            .await;
        commands.configure(data.bot_token.clone()).await;
        HttpResponse::Ok().json(serde_json::json!({"status": "configured"}))
    }

    pub async fn test_ctrl(
        service: web::Data<Arc<TelegramService>>,
        data: web::Json<TestTelegramDTO>,
    ) -> impl Responder {
        let msg = data
            .message
            .as_deref()
            .unwrap_or("Andromeda Galaxy - Telegram OK!");

        match service.send_test_message(msg).await {
            Ok(_) => HttpResponse::Ok().json(serde_json::json!({"status": "sent"})),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e})),
        }
    }

    pub fn routes(self) -> HashMap<String, Route> {
        let mut routes = HashMap::new();

        routes.insert(
            String::from("telegram/configure"),
            web::post().to(Self::configure_ctrl),
        );
        routes.insert(
            String::from("telegram/test"),
            web::post().to(Self::test_ctrl),
        );

        routes
    }
}
