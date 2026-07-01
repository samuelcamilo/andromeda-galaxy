//! Detector de rug/honeypot em background.
//!
//! Para cada mensagem registrada em `sent_messages` (no SQLite) com
//! `is_scam = false`, periodicamente:
//!   1. Consulta `https://api.honeypot.is/v2/IsHoneypot?address=<CA>`
//!   2. Se a resposta indicar honeypot OU as reservas de ETH no pair
//!      caírem para zero/quase zero (rug clássico de "tirar liquidez"),
//!      marca `is_scam = true`, re-renderiza a mensagem com prefixo
//!      "❌ #RUGGED" e blocos riscados, e edita via Telegram API.
//!
//! Espelha o fluxo do bot legado `seekers-galaxy` (`Reprocessor.ts`),
//! que combina `HoneypotIsScamService` + `editMessageText`.

use crate::http_client::HttpClient;
use crate::repositories::sqlite_repository::SqliteRepository;
use crate::services::enrichment_service::EnrichedDeploy;
use crate::services::telegram_service::TelegramService;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HONEYPOT_IS_URL: &str = "https://api.honeypot.is/v2/IsHoneypot";
/// Intervalo entre ciclos do loop. Cada ciclo processa um lote pequeno
/// para não estourar rate limit do Honeypot.is (sem auth) nem segurar
/// CPU do worker.
const TICK_INTERVAL_SECS: u64 = 30;
/// Quantos contratos por ciclo. Honeypot.is público aguenta uns 2 req/s
/// confortavelmente; com 10 por ciclo de 30s ficamos em ~0.3 req/s.
const BATCH_SIZE: u32 = 10;
/// Espaçamento entre requests do mesmo lote — evita bursts.
const PER_REQUEST_DELAY_MS: u64 = 250;

pub struct RugDetectorService {
    sqlite_repository: Arc<SqliteRepository>,
    telegram_service: Arc<TelegramService>,
}

impl RugDetectorService {
    pub fn new(
        sqlite_repository: Arc<SqliteRepository>,
        telegram_service: Arc<TelegramService>,
    ) -> Self {
        RugDetectorService {
            sqlite_repository,
            telegram_service,
        }
    }

    /// Inicia o loop em uma task tokio. Não retorna; chame a partir de
    /// `main.rs` com `tokio::spawn(detector.run())`.
    pub async fn run(self: Arc<Self>) {
        eprintln!(
            "[RUG-DETECTOR] iniciado (intervalo={}s, batch={}, delay={}ms)",
            TICK_INTERVAL_SECS, BATCH_SIZE, PER_REQUEST_DELAY_MS
        );

        let mut ticker = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
        // Primeira tick é imediata; ignoramos para dar tempo de o
        // `TelegramService.configure` rodar antes do primeiro lote.
        ticker.tick().await;

        loop {
            ticker.tick().await;

            let cfg = match self.telegram_service.current_config().await {
                Some(c) => c,
                None => {
                    // Telegram ainda não configurado — espera próximo tick.
                    continue;
                }
            };
            let (bot_token, _chat_id_cfg, bot_username) = cfg;

            let pending = match self.sqlite_repository.list_pending_scam_checks(BATCH_SIZE) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[RUG-DETECTOR] falha lendo sent_messages: {}", e);
                    continue;
                }
            };

            if pending.is_empty() {
                continue;
            }

            for row in pending {
                let now_ts = current_unix_secs();

                // marca já como verificado pra fila avançar mesmo se a
                // request abaixo travar/falhar. O round-robin natural
                // (ORDER BY last_checked_at ASC) garante revisita.
                let _ = self
                    .sqlite_repository
                    .touch_sent_message_check(&row.contract_address, now_ts);

                let scam_status = match Self::check_honeypot_is(&row.contract_address).await {
                    Some(s) => s,
                    None => {
                        // API caiu / rate limit / parse falhou — pula sem
                        // marcar como rug. Um falso positivo riscando a
                        // mensagem indevidamente é pior que esperar.
                        tokio::time::sleep(Duration::from_millis(PER_REQUEST_DELAY_MS)).await;
                        continue;
                    }
                };

                if !scam_status.is_scam {
                    tokio::time::sleep(Duration::from_millis(PER_REQUEST_DELAY_MS)).await;
                    continue;
                }

                eprintln!(
                    "[RUG-DETECTOR] {} marcado como RUGGED (honeypot={}, risk={})",
                    row.contract_address, scam_status.is_honeypot, scam_status.risk_label
                );

                // Re-renderiza a mensagem com is_scam=true.
                let mut enriched: EnrichedDeploy =
                    match serde_json::from_str(&row.enriched_json) {
                        Ok(e) => e,
                        Err(e) => {
                            eprintln!(
                                "[RUG-DETECTOR] falha desserializando enriched_json de {}: {}",
                                row.contract_address, e
                            );
                            continue;
                        }
                    };
                enriched.is_scam = true;

                let new_message = TelegramService::format_message(&enriched, &bot_username);

                if let Err(e) = TelegramService::edit_message(
                    &bot_token,
                    &row.chat_id,
                    row.message_id,
                    &new_message,
                )
                .await
                {
                    eprintln!(
                        "[RUG-DETECTOR] editMessageText falhou para {} (msg_id={}): {}",
                        row.contract_address, row.message_id, e
                    );
                    // Não marca scam — vamos tentar de novo no próximo ciclo.
                    tokio::time::sleep(Duration::from_millis(PER_REQUEST_DELAY_MS)).await;
                    continue;
                }

                if let Err(e) = self
                    .sqlite_repository
                    .mark_sent_message_scam(&row.contract_address, true)
                {
                    eprintln!(
                        "[RUG-DETECTOR] falha persistindo is_scam=true para {}: {}",
                        row.contract_address, e
                    );
                }

                tokio::time::sleep(Duration::from_millis(PER_REQUEST_DELAY_MS)).await;
            }
        }
    }

    /// Consulta `api.honeypot.is/v2/IsHoneypot?address=<CA>` e devolve o
    /// status compactado. Retorna `None` em caso de erro de rede/parse.
    async fn check_honeypot_is(contract_address: &str) -> Option<HoneypotStatus> {
        let url = format!("{}?address={}", HONEYPOT_IS_URL, contract_address);
        let client = HttpClient::new();

        let resp = client.get_client().get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }

        let body: serde_json::Value = resp.json().await.ok()?;

        // Estrutura típica: ver `Legacy/.../HoneyPotIsRepository.ts`.
        let is_honeypot = body
            .get("simulationResult")
            .and_then(|s| s.get("isHoneypot"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let risk_label = body
            .get("summary")
            .and_then(|s| s.get("risk"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Detecção de rug por reservas: WETH no pair caiu para ~zero.
        // O Honeypot.is devolve `pair.reserves0` / `pair.reserves1` como
        // strings em wei. `withToken.address` indica qual lado é o token,
        // o outro é o WETH/par. Se ambas as reservas estiverem zeradas,
        // a liquidez foi removida ("rug clássico").
        let pair = body.get("pair");
        let (eth_reserves, _token_reserves) = pair
            .map(|p| Self::extract_reserves(p, body.get("withToken")))
            .unwrap_or((0u128, 0u128));

        // Threshold conservador: < 1e15 wei = 0.001 ETH efetivamente vazio.
        // Evita falso positivo de pair recém-criado em hourly drains.
        let liquidity_drained = pair.is_some() && eth_reserves < 1_000_000_000_000_000u128;

        let risk_is_honeypot = matches!(risk_label.as_str(), "honeypot");

        let is_scam = is_honeypot || risk_is_honeypot || liquidity_drained;

        Some(HoneypotStatus {
            is_scam,
            is_honeypot,
            risk_label,
        })
    }

    /// Devolve `(eth_reserves, token_reserves)` em wei a partir do bloco
    /// `pair` da resposta do Honeypot.is. Quando os campos vêm fora do
    /// formato esperado, devolve (0, 0) — o caller trata como ruim.
    fn extract_reserves(
        pair: &serde_json::Value,
        with_token: Option<&serde_json::Value>,
    ) -> (u128, u128) {
        let reserves0 = pair
            .get("reserves0")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u128>().ok())
            .unwrap_or(0);
        let reserves1 = pair
            .get("reserves1")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u128>().ok())
            .unwrap_or(0);

        let token_addr = with_token
            .and_then(|w| w.get("address"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let token0 = pair
            .get("pair")
            .and_then(|p| p.get("token0"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        // Se o token do contrato é token0, então reserves1 é o ETH/WETH.
        if !token_addr.is_empty() && token_addr == token0 {
            (reserves1, reserves0)
        } else {
            (reserves0, reserves1)
        }
    }
}

struct HoneypotStatus {
    is_scam: bool,
    is_honeypot: bool,
    risk_label: String,
}

fn current_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
