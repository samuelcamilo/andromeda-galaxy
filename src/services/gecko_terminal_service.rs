//! GeckoTerminalService — busca o **ATH (FDV)** histórico de um token ERC-20
//! na Ethereum usando a API pública do GeckoTerminal.
//!
//! Por que GeckoTerminal e não DexScreener:
//! - O endpoint do DexScreener que tem OHLCV histórico (`io.dexscreener.com/.../bars/...`)
//!   é protegido pela Cloudflare e bloqueia requests vindas de IPs de
//!   datacenter (o servidor desse bot usa Hetzner). FlareSolverr + Playwright
//!   stealth não passam — é bloqueio por ASN, antes do challenge.
//! - GeckoTerminal expõe a mesma curva de preço (ambos derivam dos eventos
//!   `Swap` da Uniswap) numa API REST oficial sem rate limiting agressivo
//!   (30 req/min sem auth) e retorna candles diários até `limit=1000` (~2.7 anos).
//!
//! Fluxo:
//! 1. `fetch_top_pool(token)` → `tokens/<addr>/pools?page=1` (já vem ordenado
//!    por liquidez/volume; pegamos o primeiro = pool principal).
//!    Daqui já saem `fdv_usd` e `base_token_price_usd` ATUAIS — usados pra
//!    derivar `total_supply` (= `fdv_usd / price_usd`) sem chamar onchain.
//! 2. `fetch_ohlcv_daily(pool)` → array `[ts, open, high, low, close, volume]`,
//!    do candle mais recente ao mais antigo.
//! 3. ATH price = `max(high)` dos candles.
//!    ATH FDV   = `ath_price * total_supply` ≈ `ath_price * fdv_usd / price_usd`.
//!    (FDV usa supply total — equivalente ao "ATH MCAP" que o povo cita;
//!    pra tokens com burn variável fica aproximação razoável o suficiente.)

use crate::http_client::HttpClient;
use serde::{Deserialize, Serialize};

const GT_BASE: &str = "https://api.geckoterminal.com/api/v2";
const NETWORK: &str = "eth";

/// Snapshot completo de ATH de um token. Inclui o pool de referência
/// pra debug/explicação na mensagem do `/compare`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AthSnapshot {
    pub ath_price_usd: f64,
    pub ath_fdv_usd: f64,
    pub ath_at: i64, // unix seconds (UTC)
    pub current_fdv_usd: f64,
    pub current_price_usd: f64,
    /// pool address usado pra puxar o histórico
    pub pool_address: String,
}

pub struct GeckoTerminalService {
    http: HttpClient,
}

impl GeckoTerminalService {
    pub fn new() -> Self {
        Self {
            http: HttpClient::new(),
        }
    }

    /// Atalho: dado o token, retorna `AthSnapshot` completo. Faz duas
    /// requisições sequenciais (primeiro o pool, depois OHLCV) — em torno de
    /// 600-1500ms total. Caller deve cachear porque ATH histórico só sobe.
    pub async fn fetch_token_ath(&self, token_addr: &str) -> Result<AthSnapshot, String> {
        let pool = self.fetch_top_pool(token_addr).await?;
        let candles = self.fetch_ohlcv_daily(&pool.address).await?;

        if candles.is_empty() {
            return Err(format!(
                "GeckoTerminal sem candles pra pool {} ({})",
                pool.address, token_addr
            ));
        }

        // ATH price = max(high)
        let mut ath_price = 0.0f64;
        let mut ath_ts: i64 = 0;
        for c in &candles {
            if c.high > ath_price {
                ath_price = c.high;
                ath_ts = c.ts;
            }
        }

        // ATH FDV = ath_price * total_supply (constante p/ supply fixo).
        // Derivamos `total_supply` da divisão `fdv / current_price` — assim
        // não precisamos de RPC nem do `EnrichedDeploy` aqui.
        let total_supply = if pool.current_price_usd > 0.0 {
            pool.current_fdv_usd / pool.current_price_usd
        } else {
            0.0
        };
        let ath_fdv_usd = ath_price * total_supply;

        Ok(AthSnapshot {
            ath_price_usd: ath_price,
            ath_fdv_usd,
            ath_at: ath_ts,
            current_fdv_usd: pool.current_fdv_usd,
            current_price_usd: pool.current_price_usd,
            pool_address: pool.address,
        })
    }

    /// `tokens/<addr>/pools?page=1` — retorna o pool TOP (já vem ordenado
    /// por relevância pelo GeckoTerminal). Erro se o token não tem pools.
    async fn fetch_top_pool(&self, token_addr: &str) -> Result<TopPool, String> {
        let url = format!(
            "{}/networks/{}/tokens/{}/pools?page=1",
            GT_BASE,
            NETWORK,
            token_addr.to_lowercase()
        );
        let resp = self
            .http
            .get_client()
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("GT pools HTTP: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("GT pools status {}", resp.status()));
        }
        let body: serde_json::Value =
            resp.json().await.map_err(|e| format!("GT pools parse: {}", e))?;

        let arr = body
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| "GT sem data".to_string())?;
        let first = arr
            .first()
            .ok_or_else(|| format!("token {} não tem pools no GT", token_addr))?;
        let attrs = first
            .get("attributes")
            .ok_or_else(|| "GT pool sem attributes".to_string())?;

        let address = attrs
            .get("address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "GT pool sem address".to_string())?
            .to_string();

        // FDV pode vir null pra tokens muito novos/sem dados; nesse caso
        // fazemos fallback pro `market_cap_usd`. Se ambos forem null, o ATH
        // FDV não dá pra calcular — devolvemos 0 e o caller pode esconder.
        let current_fdv_usd = attrs
            .get("fdv_usd")
            .and_then(parse_f64_loose)
            .or_else(|| attrs.get("market_cap_usd").and_then(parse_f64_loose))
            .unwrap_or(0.0);

        let current_price_usd = attrs
            .get("base_token_price_usd")
            .and_then(parse_f64_loose)
            .unwrap_or(0.0);

        Ok(TopPool {
            address,
            current_fdv_usd,
            current_price_usd,
        })
    }

    /// `pools/<pool>/ohlcv/day?aggregate=1&limit=1000&currency=usd`
    /// Retorna candles do mais recente pro mais antigo.
    async fn fetch_ohlcv_daily(&self, pool_addr: &str) -> Result<Vec<Candle>, String> {
        let url = format!(
            "{}/networks/{}/pools/{}/ohlcv/day?aggregate=1&limit=1000&currency=usd",
            GT_BASE, NETWORK, pool_addr
        );
        let resp = self
            .http
            .get_client()
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("GT ohlcv HTTP: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("GT ohlcv status {}", resp.status()));
        }
        let body: serde_json::Value =
            resp.json().await.map_err(|e| format!("GT ohlcv parse: {}", e))?;

        let arr = body
            .get("data")
            .and_then(|d| d.get("attributes"))
            .and_then(|a| a.get("ohlcv_list"))
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();

        let candles = arr
            .iter()
            .filter_map(|row| {
                let row = row.as_array()?;
                if row.len() < 5 {
                    return None;
                }
                Some(Candle {
                    ts: row[0].as_i64()?,
                    high: row[2].as_f64()?,
                })
            })
            .collect();
        Ok(candles)
    }
}

#[derive(Debug)]
struct TopPool {
    address: String,
    current_fdv_usd: f64,
    current_price_usd: f64,
}

#[derive(Debug)]
struct Candle {
    ts: i64,
    high: f64,
}

/// GeckoTerminal devolve números como string (ex: `"0.00000392..."`) pra
/// preservar precisão em valores muito pequenos. Esse helper aceita ambos.
fn parse_f64_loose(v: &serde_json::Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        return s.parse::<f64>().ok();
    }
    None
}
