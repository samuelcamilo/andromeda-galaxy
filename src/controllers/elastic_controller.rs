use crate::controllers::dto::elastic_dto::{
    ChecksumCountDTO, GetLabelsFromAddressDTO, GetSignaturesDTO, ImportAnnotationsDTO,
    ImportChecksumHistoryDTO, InsertChecksumDTO, InsertSignatureDTO,
};
use crate::services::elastic::checksum_service::ChecksumService;
use crate::services::elastic::labels_service::LabelsService;
use crate::services::elastic::signatures_service::SignaturesService;
use actix_web::{web, HttpResponse, Responder, Route};
use std::collections::HashMap;
use std::sync::Arc;

pub struct ElasticController;

impl ElasticController {
    pub fn new() -> Self {
        ElasticController
    }

    pub async fn checksum_count_ctrl(
        service: web::Data<Arc<ChecksumService>>,
        data: web::Json<ChecksumCountDTO>,
    ) -> impl Responder {
        let response = service.checksum_count(&data).await;
        HttpResponse::Ok().json(response)
    }

    pub async fn get_signatures_ctrl(
        service: web::Data<Arc<SignaturesService>>,
        data: web::Json<GetSignaturesDTO>,
    ) -> impl Responder {
        let response = service.get_signatures_by_id(&data).await;
        HttpResponse::Ok().json(response)
    }

    pub async fn insert_signature_ctrl(
        service: web::Data<Arc<SignaturesService>>,
        data: web::Json<InsertSignatureDTO>,
    ) -> impl Responder {
        service.upsert(&data).await;
        HttpResponse::Ok()
    }

    pub async fn insert_checksum_ctrl(
        service: web::Data<Arc<ChecksumService>>,
        data: web::Json<InsertChecksumDTO>,
    ) -> impl Responder {
        service.upsert(&data).await;
        HttpResponse::Ok()
    }

    pub async fn update_checksum_if_necessary_ctrl(
        service: web::Data<Arc<ChecksumService>>,
        data: web::Json<InsertChecksumDTO>,
    ) -> impl Responder {
        service.update_if_necessary(&data).await;
        HttpResponse::Ok()
    }

    pub async fn get_labels_from_address_ctrl(
        path: web::Path<GetLabelsFromAddressDTO>,
        service: web::Data<Arc<LabelsService>>,
    ) -> impl Responder {
        let address = path.address.clone();

        match service.exec_by_address(address).await {
            Ok(labels) => {
                if labels.is_empty() {
                    HttpResponse::NotFound().json(serde_json::json!({ "error": "NOT_FOUND" }))
                } else {
                    HttpResponse::Ok().json(&labels[0])
                }
            }
            Err(_) => HttpResponse::InternalServerError().finish(),
        }
    }

    pub async fn import_checksum_history_ctrl(
        service: web::Data<Arc<ChecksumService>>,
        data: web::Json<ImportChecksumHistoryDTO>,
    ) -> impl Responder {
        let count = service.import_checksum_history(&data).await;
        HttpResponse::Ok().json(serde_json::json!({"imported": count}))
    }

    pub async fn clear_checksum_history_ctrl(
        service: web::Data<Arc<ChecksumService>>,
    ) -> impl Responder {
        let deleted = service.clear_checksum_history().await;
        HttpResponse::Ok().json(serde_json::json!({"deleted": deleted}))
    }

    pub async fn import_annotations_ctrl(
        service: web::Data<Arc<ChecksumService>>,
        data: web::Json<ImportAnnotationsDTO>,
    ) -> impl Responder {
        let count = service.import_annotations(&data).await;
        HttpResponse::Ok().json(serde_json::json!({"imported": count}))
    }

    pub async fn import_gas_annotations_ctrl(
        service: web::Data<Arc<ChecksumService>>,
        data: web::Json<ImportAnnotationsDTO>,
    ) -> impl Responder {
        let count = service.import_gas_annotations(&data).await;
        HttpResponse::Ok().json(serde_json::json!({"imported": count}))
    }

    pub async fn import_indicators_ctrl(
        service: web::Data<Arc<ChecksumService>>,
        data: web::Json<ImportAnnotationsDTO>,
    ) -> impl Responder {
        let count = service.import_indicators(&data).await;
        HttpResponse::Ok().json(serde_json::json!({"imported": count}))
    }

    pub fn routes(self) -> HashMap<String, Route> {
        let mut routes = HashMap::new();

        routes.insert(
            String::from("elastic/labels_from_address/{address}"),
            web::get().to(Self::get_labels_from_address_ctrl),
        );
        routes.insert(
            String::from("elastic/insert_checksum"),
            web::post().to(Self::insert_checksum_ctrl),
        );
        routes.insert(
            String::from("elastic/update_checksum"),
            web::post().to(Self::update_checksum_if_necessary_ctrl),
        );
        routes.insert(
            String::from("elastic/checksum_count"),
            web::post().to(Self::checksum_count_ctrl),
        );
        routes.insert(
            String::from("elastic/signatures"),
            web::post().to(Self::insert_signature_ctrl),
        );
        routes.insert(
            String::from("elastic/get_signatures"),
            web::post().to(Self::get_signatures_ctrl),
        );
        routes.insert(
            String::from("elastic/import_checksum_history"),
            web::post().to(Self::import_checksum_history_ctrl),
        );
        routes.insert(
            String::from("elastic/clear_checksum_history"),
            web::post().to(Self::clear_checksum_history_ctrl),
        );
        routes.insert(
            String::from("elastic/import_annotations"),
            web::post().to(Self::import_annotations_ctrl),
        );
        routes.insert(
            String::from("elastic/import_gas_annotations"),
            web::post().to(Self::import_gas_annotations_ctrl),
        );
        routes.insert(
            String::from("elastic/import_indicators"),
            web::post().to(Self::import_indicators_ctrl),
        );

        routes
    }
}
