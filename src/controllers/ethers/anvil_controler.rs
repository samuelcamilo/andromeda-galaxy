use crate::controllers::dto::ethers_dto::{AnvilGetTransactionCount, ApplyForkDTO, PathParams, RemoveDTO, SetBalanceDTO, SimulateTxDTO};
use crate::services::ethers::anvil_service::AnvilService;
use actix_web::{web, HttpResponse, Responder, Route};
use std::collections::HashMap;
use std::sync::Arc;

pub struct AnvilController {}

impl AnvilController {
    pub fn new() -> Self {
        AnvilController {}
    }

    pub async fn apply_fork_ctrl(
        data: web::Json<ApplyForkDTO>,
        service: web::Data<Arc<AnvilService>>,
    ) -> impl Responder {
        service.apply_fork(&data).await;
        HttpResponse::Ok()
    }

    pub async fn remove_fork_ctrl(
        data: web::Json<RemoveDTO>,
        service: web::Data<Arc<AnvilService>>,
    ) -> impl Responder {
        service.remove(&data).await;
        HttpResponse::Ok()
    }

    pub async fn get_transaction_count_ctrl(
        data: web::Json<AnvilGetTransactionCount>,
        service: web::Data<Arc<AnvilService>>,
    ) -> impl Responder {
        let response = service.get_transaction_count(&data).await;

        match response {
            Err(e) => HttpResponse::InternalServerError().body(format!("Erro: {}", e)),
            Ok(result) => HttpResponse::Ok().json(result),
        }
    }


    pub async fn simulate_tx_ctrl(
        data: web::Json<SimulateTxDTO>,
        service: web::Data<Arc<AnvilService>>,
    ) -> impl Responder {
        let response = service.simulate_tx(&data).await;

        match response {
            Err(e) => HttpResponse::InternalServerError().body(format!("Erro: {}", e)),
            Ok(result) => HttpResponse::Ok().json(result),
        }
    }

    pub async fn set_balance_ctrl(
        data: web::Json<SetBalanceDTO>,
        service: web::Data<Arc<AnvilService>>,
    ) -> impl Responder {
        let response = service.set_balance(&data);

        match response.await {
            Err(e) => HttpResponse::InternalServerError().body(format!("Erro: {}", e)),
            Ok(result) => HttpResponse::Ok().json(result),
        }
    }

    pub async fn call_tx_ctrl(
        data: web::Json<SimulateTxDTO>,
        service: web::Data<Arc<AnvilService>>,
    ) -> impl Responder {
        let response = service.call_transaction(&data).await;

        match response {
            Err(e) => HttpResponse::InternalServerError().body(format!("Erro: {}", e)),
            Ok(result) => HttpResponse::Ok().json(result),
        }

    }


    pub fn routes(self) -> HashMap<String, Route> {
        let mut routes = HashMap::new();

        routes.insert(String::from("anvil/create"), web::post().to(Self::apply_fork_ctrl));
        routes.insert(String::from("anvil/simulate"), web::post().to(Self::simulate_tx_ctrl));
        routes.insert(String::from("anvil/call"), web::post().to(Self::call_tx_ctrl));
        routes.insert(String::from("anvil/set_balance"), web::post().to(Self::set_balance_ctrl));
        routes.insert(String::from("anvil/transaction_count"), web::post().to(Self::get_transaction_count_ctrl));
        routes.insert(String::from("anvil/remove"), web::post().to(Self::remove_fork_ctrl));
        routes
    }

}