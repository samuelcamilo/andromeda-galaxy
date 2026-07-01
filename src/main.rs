#![recursion_limit = "256"]

mod controllers;
pub mod http_client;
mod repositories;
mod services;
mod utils;

use controllers::ethers::ethers_controller::EthersController;
use repositories::ethers::ethers_repository::EthersRepository;
use crate::repositories::sqlite_repository::SqliteRepository;

use dotenv::dotenv;
use std::sync::Arc;

use crate::controllers::cron_job_controller::CronJobController;
use crate::controllers::elastic_controller::ElasticController;
use crate::controllers::telegram_controller::TelegramController;
use crate::services::cron_job_service::CronJobService;
use crate::services::elastic::checksum_service::ChecksumService;
use crate::services::elastic::labels_service::LabelsService;
use crate::services::elastic::signatures_service::SignaturesService;
use crate::services::enrichment_service::EnrichmentService;
use crate::services::ethers::call_functions_service::CallFunctionsService;
use crate::services::ethers::call_selectors_service::CallSelectorsService;
use crate::services::ethers::ethers_service::EthersService;
use crate::services::ethers::get_logs_service::GetLogsService;
use crate::services::ethers::listen_contract_event_service::ListenContractEventsService;
use crate::services::ethers::listen_deploy_erc20_contracts_service::ListenDeployErc20ContractsService;
use crate::services::rug_detector_service::RugDetectorService;
use crate::services::telegram_service::TelegramService;
use actix_web::{middleware::Logger, web, App, HttpServer, HttpResponse};
use tokio::sync::RwLock;
use crate::controllers::ethers::anvil_controler::AnvilController;
use crate::controllers::heimdall_controller::HeimdallController;
use crate::repositories::ethers::anvil_repository::AnvilRepository;
use crate::services::anvil_simulation::AnvilSimulation;
use crate::services::ethers::anvil_service::AnvilService;

#[actix_web::main]
async fn main() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[PANIC] {}", info);
    }));

    dotenv().ok();
    env_logger::init();

    let sqlite_path = std::env::var("SQLITE_PATH").unwrap_or_else(|_| "andromeda.db".to_string());
    let sqlite_repository = Arc::new(
        SqliteRepository::new(&sqlite_path)
            .expect("Falha ao criar SqliteRepository — não é possível iniciar sem banco"),
    );

    let ethers_repository = Arc::new(RwLock::new(EthersRepository::new()));
    let anvil_repository = Arc::new(RwLock::new(AnvilRepository::new()));

    let anvil_simulation = Arc::new(AnvilSimulation::new(anvil_repository.clone()));

    let enrichment_service = Arc::new(EnrichmentService::new(sqlite_repository.clone()));
    enrichment_service.set_anvil_simulation(anvil_simulation).await;

    // Auto-configure RPC endpoint for Anvil simulation from env
    if let Ok(rpc_http) = std::env::var("RPC_HTTP_ENDPOINT") {
        eprintln!("[BOOT] RPC HTTP para Anvil configurado");
        enrichment_service.set_rpc_endpoint(rpc_http).await;
    }

    let telegram_service = Arc::new(TelegramService::new(
        enrichment_service.clone(),
        sqlite_repository.clone(),
    ));

    // Background loop que detecta rug/honeypot via Honeypot.is e reedita
    // a mensagem original com "❌ #RUGGED" + strikethrough (igual Legacy).
    let rug_detector = Arc::new(RugDetectorService::new(
        sqlite_repository.clone(),
        telegram_service.clone(),
    ));
    tokio::spawn(rug_detector.clone().run());

    let telegram_commands = Arc::new(services::telegram_commands::TelegramCommands::new(
        sqlite_repository.clone(),
        enrichment_service.clone(),
        ethers_repository.clone(),
        telegram_service.clone(),
    ));

    let listen_contract_events_service =
        Arc::new(ListenContractEventsService::new(ethers_repository.clone()));

    let labels_service = Arc::new(LabelsService::new(sqlite_repository.clone()));
    let heimdall_service = Arc::new(services::heimdall_service::HeimdallService {});
    let get_logs_service = Arc::new(GetLogsService::new(
        ethers_repository.clone(),
        sqlite_repository.clone(),
    ));

    let call_selectors_service = Arc::new(CallSelectorsService::new(ethers_repository.clone()));
    let listen_deploy_erc20_contracts_service = Arc::new(ListenDeployErc20ContractsService::new(
        ethers_repository.clone(),
        telegram_service.clone(),
    ));

    let checksum_service = Arc::new(ChecksumService::new(sqlite_repository.clone()));
    let call_functions_service = Arc::new(CallFunctionsService::new(ethers_repository.clone()));
    let ethers_service = Arc::new(EthersService::new(ethers_repository.clone()));
    let anvil_service = Arc::new(AnvilService::new(anvil_repository.clone()));
    let signatures_service = Arc::new(SignaturesService::new(sqlite_repository.clone()));
    let cronjob_service = Arc::new(CronJobService::new());

    eprintln!("[BOOT] Andromeda Galaxy iniciando na porta 8080...");

    let server = HttpServer::new(move || {
        let mut app = App::new().wrap(Logger::default());

        app = app.app_data(web::Data::new(call_functions_service.clone()));
        app = app.app_data(web::Data::new(labels_service.clone()));
        app = app.app_data(web::Data::new(get_logs_service.clone()));
        app = app.app_data(web::Data::new(listen_contract_events_service.clone()));
        app = app.app_data(web::Data::new(call_selectors_service.clone()));
        app = app.app_data(web::Data::new(checksum_service.clone()));
        app = app.app_data(web::Data::new(ethers_service.clone()));
        app = app.app_data(web::Data::new(signatures_service.clone()));
        app = app.app_data(web::Data::new(cronjob_service.clone()));
        app = app.app_data(web::Data::new(anvil_service.clone()));
        app = app.app_data(web::Data::new(heimdall_service.clone()));
        app = app.app_data(web::Data::new(telegram_service.clone()));
        app = app.app_data(web::Data::new(telegram_commands.clone()));

        app = app.app_data(web::Data::new(
            listen_deploy_erc20_contracts_service.clone(),
        ));

        app = app.route("/health", web::get().to(|| async { HttpResponse::Ok().body("OK") }));
        app = app.route("/health", web::post().to(|| async { HttpResponse::Ok().body("OK") }));

        let ethers_controller = EthersController::new();
        let elastic_controller = ElasticController::new();
        let conjob_controller = CronJobController::new();
        let anvil_controller = AnvilController::new();
        let heimdall_controller = HeimdallController::new();
        let telegram_controller = TelegramController::new();

        for (endpoint, route) in ethers_controller.routes() {
            app = app.route(&endpoint, route);
        }

        for (endpoint, route) in elastic_controller.routes() {
            app = app.route(&endpoint, route);
        }

        for (endpoint, route) in conjob_controller.routes() {
            app = app.route(&endpoint, route);
        }

        for (endpoint, route) in anvil_controller.routes() {
            app = app.route(&endpoint, route);
        }

        for (endpoint, route) in heimdall_controller.routes() {
            app = app.route(&endpoint, route);
        }

        for (endpoint, route) in telegram_controller.routes() {
            app = app.route(&endpoint, route);
        }

        app
    })
    .bind("0.0.0.0:8080");

    match server {
        Ok(srv) => {
            eprintln!("[BOOT] Servidor HTTP ativo em 0.0.0.0:8080");
            if let Err(e) = srv.run().await {
                eprintln!("[FATAL] Servidor HTTP caiu: {}", e);
            }
        }
        Err(e) => {
            eprintln!("[FATAL] Não foi possível fazer bind na porta 8080: {}", e);
            std::process::exit(1);
        }
    }
}
