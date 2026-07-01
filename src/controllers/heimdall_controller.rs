use crate::controllers::dto::heimdall_dto::GetCFGDTO;
use crate::services::heimdall_service::HeimdallService;
use actix_web::{web, HttpResponse, Responder, Route};
use std::collections::HashMap;
use std::sync::Arc;

pub struct HeimdallController;

impl HeimdallController {

    pub fn new() -> Self {HeimdallController}

    pub async fn get_cfg_ctrl(
        service: web::Data<Arc<HeimdallService>>,
        data: web::Json<GetCFGDTO>
    ) -> impl Responder{
        let bytecode = data.bytecode.clone();

        match service.get_cfg_as_json(bytecode).await {
            Ok(cfg) => {
                HttpResponse::Ok().json(cfg)
            },
            Err(_) => HttpResponse::InternalServerError().finish(),
        }
    }

    pub fn routes(self) -> HashMap<String, Route> {
        let mut routes = HashMap::new();

        routes.insert(
            String::from("heimdall/cfg"),
            web::post().to(Self::get_cfg_ctrl),
        );

        routes
    }
}