use actix_web::{web, HttpResponse, Responder, Route};
use std::collections::HashMap;
use std::sync::Arc;
use crate::controllers::dto::ethers_dto::{ApplyRpcDTO, CallFunctionsDTO, CallSelectorsDTO, GetCodeDTO, GetLogsDTO, GetTransactionCountDTO, GetTransactionDTO, ListenContractEventsDTO, ListenDeployErc20ContractsDTO, PathParams};
use crate::services::ethers::call_functions_service::CallFunctionsService;
use crate::services::ethers::call_selectors_service::CallSelectorsService;
use crate::services::ethers::ethers_service::EthersService;
use crate::services::ethers::get_logs_service::GetLogsService;
use crate::services::ethers::listen_contract_event_service::ListenContractEventsService;
use crate::services::ethers::listen_deploy_erc20_contracts_service::ListenDeployErc20ContractsService;

pub struct EthersController;

impl EthersController {
    pub fn new() -> Self {
        EthersController {}
    }

    pub async fn get_logs_ctrl(
        path: web::Path<PathParams>,
        data: web::Json<GetLogsDTO>,
        service: web::Data<Arc<GetLogsService>>,
    ) -> impl Responder {
        let user_id = path.id;

        service.exec(user_id,&data).await;
        HttpResponse::Ok()
    }

    pub async fn call_selectors_ctrl(
        path: web::Path<PathParams>,
        data: web::Json<CallSelectorsDTO>,
        service: web::Data<Arc<CallSelectorsService>>,
    ) -> impl Responder {
        let user_id = path.id;
        let response = service.exec(user_id,&data).await;
        HttpResponse::Ok().json(response)
    }


    pub async fn get_code_ctrl(
        path: web::Path<PathParams>,
        params: web::Path<GetCodeDTO>,
        service: web::Data<Arc<EthersService>>,
    ) -> impl Responder {
        match service.get_code(path.id, &params).await {
            Ok(response) => HttpResponse::Ok().json(response),
            Err(e) => HttpResponse::InternalServerError().body(e),
        }
    }

    pub async fn get_balance_ctrl(
        path: web::Path<PathParams>,
        params: web::Query<GetTransactionCountDTO>,
        service: web::Data<Arc<EthersService>>,
    ) -> impl Responder {
        match service.get_balance(path.id, &params).await {
            Ok(response) => HttpResponse::Ok().json(response),
            Err(e) => HttpResponse::InternalServerError().body(e),
        }
    }

    pub async fn get_transaction_count_ctrl(
        path: web::Path<PathParams>,
        params: web::Query<GetTransactionCountDTO>,
        service: web::Data<Arc<EthersService>>,
    ) -> impl Responder {
        match service.get_transaction_count(path.id, &params).await {
            Ok(response) => HttpResponse::Ok().json(response),
            Err(e) => HttpResponse::InternalServerError().body(e),
        }
    }

    pub async fn listen_contract_events_ctrl(
        path: web::Path<PathParams>,
        data: web::Json<ListenContractEventsDTO>,
        service: web::Data<Arc<ListenContractEventsService>>,
    ) -> impl Responder {
        let user_id = path.id;
        service.exec(user_id,&data).await;
        HttpResponse::Ok()
    }

    pub async fn call_functions_ctrl(
        path: web::Path<PathParams>,
        data: web::Json<CallFunctionsDTO>,
        service: web::Data<Arc<CallFunctionsService>>
    ) -> impl Responder {
        let id = path.id.clone();
        let service_response = service.exec(id,&data).await;
        HttpResponse::Ok().json(service_response)
    }

    pub async fn listen_deploy_erc20_contracts_ctrl(
        path: web::Path<PathParams>,
        data: web::Json<ListenDeployErc20ContractsDTO>,
        service: web::Data<Arc<ListenDeployErc20ContractsService>>
    ) -> impl Responder {
        let id = path.id.clone();

        service.exec(id,&data).await;
        HttpResponse::Ok()
    }

    pub async fn get_transaction_ctrl(
        path: web::Path<PathParams>,
        data: web::Path<GetTransactionDTO>,
        service: web::Data<Arc<EthersService>>,
    ) -> impl Responder {
        let hash = match data.transaction_hash.parse() {
            Ok(h) => h,
            Err(_) => return HttpResponse::BadRequest().body("Hash inválido"),
        };
        match service.get_transaction(path.id, hash).await {
            Ok(Some(tx)) => HttpResponse::Ok().json(tx),
            Ok(None) => HttpResponse::NotFound().body("Transação não encontrada"),
            Err(e) => HttpResponse::InternalServerError().body(e),
        }
    }

    pub async fn apply_rpc_ctrl(
        path: web::Path<PathParams>,
        data: web::Json<ApplyRpcDTO>,
        service: web::Data<Arc<EthersService>>,
    ) -> impl Responder {
        match service.apply_rpc(path.id, &data).await {
            Ok(()) => HttpResponse::Ok().finish(),
            Err(e) => HttpResponse::InternalServerError().body(e),
        }
    }

    pub fn routes(self) -> HashMap<String, Route> {
        let mut routes = HashMap::new();

        routes.insert(String::from("ethers/{id}/apply_rpc"), web::post().to(Self::apply_rpc_ctrl));
        routes.insert(String::from("ethers/{id}/call_functions"), web::post().to(Self::call_functions_ctrl));
        routes.insert(String::from("ethers/{id}/get_logs"), web::post().to(Self::get_logs_ctrl));
        routes.insert(String::from("ethers/{id}/listen_deploy_erc20"), web::post().to(Self::listen_deploy_erc20_contracts_ctrl));
        routes.insert(String::from("ethers/{id}/listen_contract_events"), web::post().to(Self::listen_contract_events_ctrl));
        routes.insert(String::from("ethers/{id}/get_code/{address}"), web::get().to(Self::get_code_ctrl));
        routes.insert(String::from("ethers/{id}/get_transaction_count"), web::get().to(Self::get_transaction_count_ctrl));
        routes.insert(String::from("ethers/{id}/get_balance"), web::get().to(Self::get_balance_ctrl));
        routes.insert(String::from("ethers/{id}/call_selectors"), web::post().to(Self::call_selectors_ctrl));
        routes.insert(String::from("ethers/{id}/get_transaction/{transaction_hash}"), web::get().to(Self::get_transaction_ctrl));

        routes
    }
}
