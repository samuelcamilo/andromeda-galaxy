use crate::http_client::HttpClient;
use crate::repositories::sqlite_repository::SqliteRepository;
use crate::services::ethers::find_deploys::find_deploys_service::FindDeploysPayload;
use crate::services::gecko_terminal_service::GeckoTerminalService;
use crate::services::heimdall_service::HeimdallService;
use ethers::abi::{decode, ParamType};
use ethers::middleware::Middleware;
use ethers::prelude::{Provider, Ws, U256, U64};
use ethers::types::{
    BlockId, BlockNumber, Bytes, NameOrAddress, TransactionRequest, H160,
};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::utils::{format_ether, hex, keccak256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

const SELECTOR_NAME: &str = "06fdde03";
const SELECTOR_SYMBOL: &str = "95d89b41";
const SELECTOR_DECIMALS: &str = "313ce567";
const SELECTOR_TOTAL_SUPPLY: &str = "18160ddd";
const SELECTOR_OWNER: &str = "8da5cb5b";

/// Seletores ERC-20/Uniswap padrão que **não** dão sinal de similaridade —
/// quase todo token tem `transfer/approve/balanceOf/etc`. Filtrar isso é o
/// que diferencia "tokens são parecidos" de "tokens são clones".
///
/// Mesma lista usada em `extract_functions_from_bytecode`; reaproveitada
/// aqui pra deixar o Jaccard do `/compare` consistente entre target e
/// candidato (espelha o `SignaturesUtil.ignoreSignatures` do Legacy).
const STANDARD_ERC20_SELECTORS: &[&str] = &[
    "313ce567", "06fdde03", "95d89b41",
    "18160ddd", "8da5cb5b", "3eaaf86b", "59d0f713",
    "3fc8cef3", "e96fada2", "1694505e", "902d55a5", "893d20e8",
    "3b97e856", "dd62ed3e", "23b872dd", "70a08231", "095ea7b3", "a9059cbb", "715018a6",
    "c45a0155", "ad5c4648", "f305d719", "791ac947", "751039fc",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichedDeploy {
    pub contract_address: String,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub decimals: Option<u8>,
    pub total_supply: Option<String>,
    pub total_supply_raw: Option<U256>,
    pub buy_fee: Option<String>,
    pub sell_fee: Option<String>,
    pub max_tx: Option<String>,
    pub max_tx_pct: Option<String>,
    pub max_wallet: Option<String>,
    pub max_wallet_pct: Option<String>,
    pub deployer: String,
    pub deployer_balance: String,
    pub deployer_nonce: String,
    pub funding_source: Option<String>,
    pub funding_amount: Option<String>,
    pub buy_gas: Option<u64>,
    pub sell_gas: Option<u64>,
    pub bytecode_checksums: Vec<ChecksumEntry>,
    pub function_checksums: Vec<ChecksumEntry>,
    pub contract_functions: Vec<ContractFunction>,
    pub is_renounced: bool,
    pub owner_address: Option<String>,
    pub is_verified: Option<bool>,
    pub verified_compiler: Option<String>,
    /// Links extraídos do SourceCode quando o contrato está verificado
    /// (twitter, telegram, web, etc). Espelha o `processSocials` do Legacy
    /// (`seekers-galaxy/src/populators/Contract.ts` + `StringUtils.extractLinks`).
    /// Default em deserialização para manter compat com mensagens persistidas
    /// antes deste campo existir.
    #[serde(default)]
    pub socials: ContractSocials,
    pub block_number: Option<U64>,
    pub pair_address: Option<String>,
    pub pair_buy_gas: Option<u64>,
    pub pair_sell_gas: Option<u64>,
    pub honeypot_result: Option<HoneypotResult>,
    pub annotation: Option<String>,
    pub gas_annotation: Option<String>,
    pub funding_source_full: Option<String>,
    /// `true` quando o contrato foi rugado (liquidez removida) ou virou
    /// honeypot. Inicialmente `false` no envio do deploy; é atualizado
    /// posteriormente pelo `RugDetectorService` que reedita a mensagem.
    #[serde(default)]
    pub is_scam: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChecksumEntry {
    pub label: String,
    pub hex: String,
    pub scam_count: u64,
    pub total_count: u64,
    pub percentage: String,
    pub is_sub: bool,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractFunction {
    pub selector: String,
    pub name: Option<String>,
    pub return_value: Option<String>,
    pub return_pct: Option<String>,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoneypotResult {
    pub can_buy: bool,
    pub can_sell: bool,
}

/// Redes sociais extraídas do source code verificado do contrato.
/// Cada campo guarda apenas o **primeiro** link encontrado (mesmo
/// comportamento do `extractLinks` do Legacy, que faz `links[0]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContractSocials {
    pub telegram: Option<String>,
    pub twitter: Option<String>,
    pub web: Option<String>,
    pub instagram: Option<String>,
    pub tiktok: Option<String>,
    pub youtube: Option<String>,
}

/// Um candidato retornado por `/compare <addr>` — entrada do histórico
/// (`sent_messages`) que tem similaridade alta com o contrato consultado.
#[derive(Debug, Clone, Serialize)]
pub struct SimilarMatch {
    pub address: String,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub bc_checksum: Option<String>,
    pub fn_checksum: Option<String>,
    pub bc_checksum_match: bool,
    pub fn_checksum_match: bool,
    pub selectors_shared: usize,
    pub selectors_total_target: usize,
    pub selectors_total_candidate: usize,
    pub selectors_jaccard: f64,
    pub score: f64,
    pub is_scam: bool,
    /// All-Time High FDV em USD (via GeckoTerminal). `None` quando o
    /// token não tem pools no GT (muito novo / não negociado em DEX
    /// suportado), ou quando a chamada falhou (rate limit etc).
    pub ath_market_cap_usd: Option<f64>,
    /// Unix seconds (UTC) de quando o ATH foi atingido.
    pub ath_at: Option<i64>,
}

/// Resultado de `/compare` entre dois contratos. Cada métrica é calculada
/// localmente a partir do bytecode runtime (sem depender de Etherscan), e
/// `overall_similarity` é uma combinação ponderada — ver `EnrichmentService::compare`.
#[derive(Debug, Clone, Serialize)]
pub struct CompareReport {
    pub address_a: String,
    pub address_b: String,
    pub bytecode_len_a: usize,
    pub bytecode_len_b: usize,
    pub bytecode_identical: bool,
    pub bc_checksum_a: String,
    pub bc_checksum_b: String,
    pub bc_checksum_match: bool,
    pub fn_checksum_a: String,
    pub fn_checksum_b: String,
    pub fn_checksum_match: bool,
    pub selectors_total_a: usize,
    pub selectors_total_b: usize,
    pub selectors_shared: usize,
    pub selectors_jaccard: f64,
    pub opcode_jaccard: f64,
    pub length_similarity: f64,
    pub overall_similarity: f64,
}

impl ChecksumEntry {
    pub fn risk_emoji(&self) -> &str {
        ""
    }
}

pub struct EnrichmentService {
    sqlite_repository: Arc<SqliteRepository>,
    etherscan_api_key: Arc<RwLock<Option<String>>>,
    rpc_endpoint: Arc<RwLock<Option<String>>>,
    anvil_simulation: Arc<RwLock<Option<Arc<crate::services::anvil_simulation::AnvilSimulation>>>>,
    gecko_terminal: Arc<GeckoTerminalService>,
}

impl EnrichmentService {
    pub fn new(sqlite_repository: Arc<SqliteRepository>) -> Self {
        EnrichmentService {
            sqlite_repository,
            etherscan_api_key: Arc::new(RwLock::new(None)),
            rpc_endpoint: Arc::new(RwLock::new(None)),
            anvil_simulation: Arc::new(RwLock::new(None)),
            gecko_terminal: Arc::new(GeckoTerminalService::new()),
        }
    }

    pub async fn set_etherscan_key(&self, key: String) {
        *self.etherscan_api_key.write().await = Some(key);
    }

    pub async fn set_rpc_endpoint(&self, endpoint: String) {
        *self.rpc_endpoint.write().await = Some(endpoint);
    }

    pub async fn set_anvil_simulation(&self, sim: Arc<crate::services::anvil_simulation::AnvilSimulation>) {
        *self.anvil_simulation.write().await = Some(sim);
    }

    /// Compara dois contratos pelo bytecode runtime, retornando várias
    /// métricas de similaridade + um score consolidado (0.0 .. 1.0).
    ///
    /// Estratégia:
    ///   1. Igualdade exata do bytecode runtime → 100%.
    ///   2. Checksum Heimdall do Bytecode (mesmo algoritmo de
    ///      `checksum_by_opcode`, idem Legacy) — se bater, business logic
    ///      é a mesma e o score parte de 95%, somando até 100% conforme o
    ///      Jaccard de seletores.
    ///   3. Caso contrário, score ponderado:
    ///      - Jaccard de seletores (PUSH4 do bytecode) — peso 0.45
    ///      - Jaccard de 4-shingles de opcodes (skeleton) — peso 0.35
    ///      - Similaridade de tamanho                     — peso 0.20
    ///
    /// Requer `set_rpc_endpoint` configurado (HTTP RPC) — usa `eth_getCode`
    /// via JSON-RPC pra evitar abrir provider WS dedicado.
    pub async fn compare(&self, addr_a: &str, addr_b: &str) -> Result<CompareReport, String> {
        let endpoint = {
            let lock = self.rpc_endpoint.read().await;
            lock.clone()
                .ok_or_else(|| "RPC HTTP endpoint não configurado".to_string())?
        };

        let norm_a = Self::normalize_address(addr_a)?;
        let norm_b = Self::normalize_address(addr_b)?;

        let (code_a, code_b) = tokio::join!(
            Self::fetch_runtime_code(&endpoint, &norm_a),
            Self::fetch_runtime_code(&endpoint, &norm_b),
        );
        let code_a = code_a?;
        let code_b = code_b?;

        let clean_a = code_a.trim_start_matches("0x").to_lowercase();
        let clean_b = code_b.trim_start_matches("0x").to_lowercase();

        if clean_a.is_empty() || clean_a == "0" {
            return Err(format!("{} não tem bytecode (não é contrato ou foi self-destructed)", norm_a));
        }
        if clean_b.is_empty() || clean_b == "0" {
            return Err(format!("{} não tem bytecode (não é contrato ou foi self-destructed)", norm_b));
        }

        let bytecode_identical = clean_a == clean_b;
        let len_a = clean_a.len() / 2;
        let len_b = clean_b.len() / 2;
        let max_len = len_a.max(len_b).max(1);
        let length_similarity = 1.0
            - ((len_a as f64 - len_b as f64).abs() / max_len as f64);

        let sel_a = Self::extract_selectors_from_bytecode(&clean_a);
        let sel_b = Self::extract_selectors_from_bytecode(&clean_b);
        let set_a: std::collections::HashSet<&String> = sel_a.iter().collect();
        let set_b: std::collections::HashSet<&String> = sel_b.iter().collect();
        let selectors_shared = set_a.intersection(&set_b).count();
        let selectors_union = set_a.union(&set_b).count();
        let selectors_jaccard = if selectors_union == 0 {
            1.0
        } else {
            selectors_shared as f64 / selectors_union as f64
        };

        // Heimdall pode ser caro: paraleliza os 2 checksums.
        let (bc_ck_a, bc_ck_b) = tokio::join!(
            Self::checksum_by_opcode(&code_a),
            Self::checksum_by_opcode(&code_b),
        );
        let bc_checksum_match = bc_ck_a == bc_ck_b;

        let sel_strs_a: Vec<&str> = sel_a.iter().map(|s| s.as_str()).collect();
        let sel_strs_b: Vec<&str> = sel_b.iter().map(|s| s.as_str()).collect();
        let fn_ck_a = Self::composed_keccak256(&sel_strs_a);
        let fn_ck_b = Self::composed_keccak256(&sel_strs_b);
        let fn_checksum_match = fn_ck_a == fn_ck_b;

        let opcode_jaccard = Self::opcode_skeleton_jaccard(&clean_a, &clean_b);

        let overall_similarity = if bytecode_identical {
            1.0
        } else if bc_checksum_match {
            // Mesma lógica Heimdall (immutables/constructor args podem variar
            // o bytecode mas mantêm o checksum). Damos piso de 95% e somamos
            // até 5% conforme alinhamento exato dos seletores.
            (0.95 + selectors_jaccard * 0.05).min(1.0)
        } else {
            selectors_jaccard * 0.45
                + opcode_jaccard * 0.35
                + length_similarity * 0.20
        };

        Ok(CompareReport {
            address_a: norm_a,
            address_b: norm_b,
            bytecode_len_a: len_a,
            bytecode_len_b: len_b,
            bytecode_identical,
            bc_checksum_a: bc_ck_a,
            bc_checksum_b: bc_ck_b,
            bc_checksum_match,
            fn_checksum_a: fn_ck_a,
            fn_checksum_b: fn_ck_b,
            fn_checksum_match,
            selectors_total_a: sel_a.len(),
            selectors_total_b: sel_b.len(),
            selectors_shared,
            selectors_jaccard,
            opcode_jaccard,
            length_similarity,
            overall_similarity,
        })
    }

    /// Replay manual da pipeline de enriquecimento para um contrato qualquer.
    /// Usado pelo `/check <ca>` — permite reaproveitar o mesmo `format_message`
    /// do TelegramService pra mostrar exatamente como o card do bot aparece.
    ///
    /// Resolve o deployer + bloco de criação via Etherscan
    /// (`getcontractcreation`). Se não tiver chave configurada ou a API
    /// não devolver, cai num fallback (`from = 0x0`, `block_number = None`)
    /// — algumas métricas perdem precisão, mas o card é gerado mesmo assim.
    pub async fn enrich_by_address(
        &self,
        provider: Arc<Provider<Ws>>,
        addr: &str,
    ) -> Result<EnrichedDeploy, String> {
        let norm = Self::normalize_address(addr)?;
        let h160_addr: H160 = norm
            .parse()
            .map_err(|e| format!("endereço inválido: {}", e))?;

        // Bytecode runtime — espelha o que o `find_on_receipt` faz quando
        // detecta um deploy: usa `provider.get_code(...).to_string()` como
        // o "input" do payload.
        let bytecode = provider
            .get_code(h160_addr, None)
            .await
            .map_err(|e| format!("eth_getCode falhou: {}", e))?;
        let bytecode_str = format!("{:?}", bytecode); // formata como "0x..."
        let runtime_hex = if bytecode_str.starts_with("0x") {
            bytecode_str.clone()
        } else {
            format!("0x{}", bytecode_str)
        };
        let clean_check = runtime_hex.trim_start_matches("0x");
        if clean_check.is_empty() {
            return Err(format!(
                "{} não tem bytecode (não é contrato ou foi self-destructed)",
                norm
            ));
        }

        let (deployer_h160, block_number) = self.fetch_contract_creation(&norm).await
            .unwrap_or_else(|e| {
                eprintln!("[CHECK] fallback ({}); seguindo com deployer 0x0", e);
                (H160::zero(), None)
            });

        let payload = FindDeploysPayload {
            contract_address: h160_addr,
            from: deployer_h160,
            input: runtime_hex,
            block_number,
        };

        Ok(self.enrich(provider, &payload).await)
    }

    /// Consulta `module=contract&action=getcontractcreation` do Etherscan v2
    /// pra descobrir o deployer e (quando disponível) o block_number do
    /// contrato. Usado pelo `/check`.
    async fn fetch_contract_creation(
        &self,
        address: &str,
    ) -> Result<(H160, Option<U64>), String> {
        let api_key = self.etherscan_api_key.read().await;
        let key = match api_key.as_ref() {
            Some(k) => k.clone(),
            None => return Err("etherscan key não configurada".to_string()),
        };

        let url = format!(
            "https://api.etherscan.io/v2/api?chainid=1&module=contract&action=getcontractcreation&contractaddresses={}&apikey={}",
            address, key
        );
        let client = HttpClient::new();
        let resp = client
            .get_client()
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("etherscan HTTP: {}", e))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("etherscan parse: {}", e))?;

        let arr = body
            .get("result")
            .and_then(|r| r.as_array())
            .ok_or_else(|| {
                format!(
                    "etherscan sem result: {}",
                    body.get("message").and_then(|m| m.as_str()).unwrap_or("?")
                )
            })?;
        let first = arr
            .first()
            .ok_or_else(|| "etherscan retornou lista vazia".to_string())?;

        let deployer_str = first["contractCreator"]
            .as_str()
            .ok_or_else(|| "etherscan sem contractCreator".to_string())?;
        let deployer: H160 = deployer_str
            .parse()
            .map_err(|e| format!("contractCreator inválido: {}", e))?;

        // `blockNumber` pode vir como string decimal ("12345") na v2.
        let block_number = first["blockNumber"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .map(U64::from);

        Ok((deployer, block_number))
    }

    /// Modo "busca" do `/compare`: dado um único endereço, varre todos os
    /// deploys já notificados (tabela `sent_messages`) e retorna os top-N
    /// mais similares, ranqueados por:
    ///
    ///   1. Match exato do **bytecode checksum** Heimdall → score 1.0
    ///      (mesma business logic — equivalente a "idêntico" em termos de
    ///      lógica, mesmo que immutables/constructor args mudem).
    ///   2. Match do **functions checksum** → 0.85 + jaccard×0.15
    ///      (mesmo conjunto de seletores).
    ///   3. Caso contrário → jaccard puro dos seletores.
    ///
    /// Usa apenas o que está em `enriched_json`; **não** chama RPC pra
    /// cada candidato (impossível em escala). O alvo (`addr`) é
    /// resolvido via RPC só uma vez.
    pub async fn find_similar(
        &self,
        addr: &str,
        top_n: usize,
    ) -> Result<(String, Vec<SimilarMatch>), String> {
        let endpoint = {
            let lock = self.rpc_endpoint.read().await;
            lock.clone()
                .ok_or_else(|| "RPC HTTP endpoint não configurado".to_string())?
        };

        let norm = Self::normalize_address(addr)?;
        let code = Self::fetch_runtime_code(&endpoint, &norm).await?;
        let clean = code.trim_start_matches("0x").to_lowercase();
        if clean.is_empty() || clean == "0" {
            return Err(format!(
                "{} não tem bytecode (não é contrato ou foi self-destructed)",
                norm
            ));
        }

        // ALL PUSH4 brutos — usados só pra computar o `fn_checksum` que tem
        // que bater bit-a-bit com o que está armazenado nos enriched_json
        // (gerado da mesma forma em `build_all_checksums`). Não usamos esse
        // conjunto pro Jaccard porque mistura ruído (PUSH4 que aparecem como
        // operandos / constantes não-dispatcher).
        let target_selectors_all = Self::extract_selectors_from_bytecode(&clean);
        let target_sel_strs_all: Vec<&str> =
            target_selectors_all.iter().map(|s| s.as_str()).collect();
        let target_fn_ck = Self::composed_keccak256(&target_sel_strs_all);
        let target_bc_ck = Self::checksum_by_opcode(&code).await;

        // Versão filtrada (DUP1+PUSH4 + remove ERC-20 padrão) — match
        // exato com o que `extract_functions_from_bytecode` produz e
        // serializa em `contract_functions[*].selector` no DB. É o
        // conjunto certo pra Jaccard "semântico".
        let target_filtered = Self::extract_dispatcher_selectors_filtered(&clean);
        let target_set: std::collections::HashSet<&String> =
            target_filtered.iter().collect();

        let rows = self
            .sqlite_repository
            .list_all_enriched_brief()
            .map_err(|e| format!("erro lendo histórico: {}", e))?;

        let mut matches: Vec<SimilarMatch> = Vec::new();

        for (cand_addr, json) in rows {
            // Pula o próprio contrato consultado (caso já esteja na base).
            if cand_addr.eq_ignore_ascii_case(&norm) {
                continue;
            }

            let parsed: serde_json::Value = match serde_json::from_str(&json) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let cand_bc = parsed["bytecode_checksums"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|c| c["hex"].as_str())
                .map(|s| s.to_string());

            let cand_fn = parsed["function_checksums"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|c| c["hex"].as_str())
                .map(|s| s.to_string());

            // Seletores armazenados em `contract_functions[*].selector`.
            let cand_selectors: Vec<String> = parsed["contract_functions"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|f| f["selector"].as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            let cand_set: std::collections::HashSet<&String> =
                cand_selectors.iter().collect();

            let shared = target_set.intersection(&cand_set).count();
            let union = target_set.union(&cand_set).count();
            let jaccard = if union == 0 { 0.0 } else { shared as f64 / union as f64 };

            let bc_match = cand_bc
                .as_deref()
                .map(|c| c.eq_ignore_ascii_case(&target_bc_ck))
                .unwrap_or(false);
            let fn_match = cand_fn
                .as_deref()
                .map(|c| c.eq_ignore_ascii_case(&target_fn_ck))
                .unwrap_or(false);

            let score = if bc_match {
                1.0
            } else if fn_match {
                (0.85 + jaccard * 0.15).min(1.0)
            } else {
                jaccard
            };

            // Filtro de ruído: ignora candidatos com 0 seletores em comum
            // E sem nenhum match de checksum — não vale a pena listar.
            if !bc_match && !fn_match && shared == 0 {
                continue;
            }

            matches.push(SimilarMatch {
                address: cand_addr,
                name: parsed["name"].as_str().map(|s| s.to_string()),
                symbol: parsed["symbol"].as_str().map(|s| s.to_string()),
                bc_checksum: cand_bc,
                fn_checksum: cand_fn,
                bc_checksum_match: bc_match,
                fn_checksum_match: fn_match,
                selectors_shared: shared,
                selectors_total_target: target_filtered.len(),
                selectors_total_candidate: cand_selectors.len(),
                selectors_jaccard: jaccard,
                score,
                is_scam: parsed["is_scam"].as_bool().unwrap_or(false),
                ath_market_cap_usd: None,
                ath_at: None,
            });
        }

        // Ordem decrescente por score; desempate por seletores compartilhados.
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.selectors_shared.cmp(&a.selectors_shared))
        });
        matches.truncate(top_n);

        // Enriquece com ATH (FDV) via GeckoTerminal — só pros top-N
        // sobreviventes. Cache em DB primeiro pra evitar chamada repetida.
        self.populate_ath(&mut matches).await;

        Ok((norm, matches))
    }

    /// Quanto tempo guardar um "negativo" (token sem dados no GeckoTerminal)
    /// antes de tentar de novo. Tokens recém-deployados podem aparecer no
    /// índice depois de algumas horas; tokens rugados velhos provavelmente
    /// nunca terão dados. 7 dias é compromisso razoável.
    const ATH_NEGATIVE_TTL_SECS: i64 = 7 * 86_400;

    /// Preenche `ath_market_cap_usd` e `ath_at` em cada `SimilarMatch`.
    /// Estratégia em 2 fases:
    ///   1. Cache lookup em `sent_messages.ath_*` (instantâneo, em batch).
    ///      Se `ath_market_cap_usd > 0`  → match positivo, usa direto.
    ///      Se `last_dex_check_at > now-7d` e `ath_market_cap_usd == 0`
    ///         → negative cache, pula fetch e devolve `None`.
    ///   2. Pros que sobraram, dispara chamadas em paralelo ao GeckoTerminal
    ///      e persiste o resultado (positivo OU negativo). Falhas de rede
    ///      são silenciosas — o card sai sem ATH naquele item.
    async fn populate_ath(&self, matches: &mut [SimilarMatch]) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Fase 1: cache (positivo ou negativo).
        let mut needs_fetch: Vec<usize> = Vec::new();
        for (i, m) in matches.iter_mut().enumerate() {
            match self.sqlite_repository.get_ath(&m.address) {
                Ok(Some(row)) if row.ath_market_cap_usd > 0.0 => {
                    m.ath_market_cap_usd = Some(row.ath_market_cap_usd);
                    m.ath_at = Some(row.ath_at);
                }
                Ok(Some(row))
                    if row.last_dex_check_at > 0
                        && (now - row.last_dex_check_at) < Self::ATH_NEGATIVE_TTL_SECS =>
                {
                    // Negative cache válido — token sem dados no GT, pula.
                }
                _ => needs_fetch.push(i),
            }
        }
        if needs_fetch.is_empty() {
            return;
        }

        // Fase 2: fetch em paralelo dos que sobraram.
        let pending: Vec<(usize, String)> =
            needs_fetch.iter().map(|i| (*i, matches[*i].address.clone())).collect();

        let mut futs = Vec::new();
        for (idx, addr) in pending.iter() {
            let gt = self.gecko_terminal.clone();
            let addr_cloned = addr.clone();
            let idx = *idx;
            futs.push(tokio::spawn(async move {
                let res = gt.fetch_token_ath(&addr_cloned).await;
                (idx, addr_cloned, res)
            }));
        }
        for fut in futs {
            if let Ok((idx, addr, res)) = fut.await {
                match res {
                    Ok(snap) => {
                        if idx < matches.len() {
                            matches[idx].ath_market_cap_usd = Some(snap.ath_fdv_usd);
                            matches[idx].ath_at = Some(snap.ath_at);
                        }
                        let _ = self.sqlite_repository.set_ath(
                            &addr,
                            snap.ath_fdv_usd,
                            snap.ath_price_usd,
                            snap.ath_at,
                            now,
                        );
                    }
                    Err(e) => {
                        // Negative cache — grava com mcap=0 e last_dex_check_at=now
                        // pra não bater no GT de novo até `ATH_NEGATIVE_TTL_SECS`.
                        eprintln!("[ATH] GeckoTerminal sem dados pra {} ({})", addr, e);
                        let _ = self.sqlite_repository.set_ath(&addr, 0.0, 0.0, 0, now);
                    }
                }
            }
        }
    }

    /// Versão "limpa" da extração de seletores — espelha exatamente a
    /// lógica de `extract_functions_from_bytecode`:
    ///   1. Procura padrão `DUP1+PUSH4` (`8063`) que é como o solc gera o
    ///      dispatcher. Pega só seletores que estão sendo realmente
    ///      comparados em runtime, descartando PUSH4 que aparecem como
    ///      constantes/imediatos no bytecode.
    ///   2. Fallback pra `PUSH4` puro caso o dispatcher não use o padrão
    ///      8063 (compiladores antigos / contratos com montagem manual).
    ///   3. Filtra os ~25 seletores ERC-20/Uniswap padrão que toda token
    ///      tem (`transfer`, `approve`, `balanceOf`, etc) — sem isso, todo
    ///      token bate ~80% Jaccard com qualquer outro só por ser ERC-20.
    ///
    /// Resultado: o conjunto que **realmente** identifica a personalidade
    /// do contrato, e que bate 1:1 com `contract_functions[*].selector`
    /// guardado no `enriched_json` dos candidatos.
    fn extract_dispatcher_selectors_filtered(clean_bytecode: &str) -> Vec<String> {
        let bytes = match hex::decode(clean_bytecode) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };

        let mut selectors: Vec<String> = Vec::new();
        let mut i = 0;
        while i + 6 < bytes.len() {
            // DUP1 (0x80) + PUSH4 (0x63) — padrão do dispatcher do solc.
            if bytes[i] == 0x80 && bytes[i + 1] == 0x63 {
                let selector = format!(
                    "{:02x}{:02x}{:02x}{:02x}",
                    bytes[i + 2], bytes[i + 3], bytes[i + 4], bytes[i + 5]
                );
                if !selectors.iter().any(|s| s == &selector) {
                    selectors.push(selector);
                }
                i += 6;
            } else {
                i += 1;
            }
        }

        // Fallback pra PUSH4 puro se nada bateu com 8063.
        if selectors.is_empty() {
            let mut i = 0;
            while i + 5 < bytes.len() {
                if bytes[i] == 0x63 {
                    let selector = format!(
                        "{:02x}{:02x}{:02x}{:02x}",
                        bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]
                    );
                    if !selectors.iter().any(|s| s == &selector) {
                        selectors.push(selector);
                    }
                    i += 5;
                } else {
                    i += 1;
                }
            }
        }

        selectors
            .into_iter()
            .filter(|s| !STANDARD_ERC20_SELECTORS.contains(&s.as_str()))
            .map(|s| format!("0x{}", s))
            .collect()
    }

    fn normalize_address(addr: &str) -> Result<String, String> {
        let trimmed = addr.trim();
        let stripped = trimmed.trim_start_matches("0x").trim_start_matches("0X");
        if stripped.len() != 40 {
            return Err(format!("endereço inválido (40 hex esperados): {}", addr));
        }
        if !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("endereço com caracteres não-hex: {}", addr));
        }
        Ok(format!("0x{}", stripped.to_lowercase()))
    }

    /// `eth_getCode` via JSON-RPC HTTP. Retorna a string `0x...` (vazia se
    /// o endereço for EOA / contrato destruído).
    async fn fetch_runtime_code(endpoint: &str, address: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_getCode",
            "params": [address, "latest"],
        });
        let client = HttpClient::new();
        let resp = client
            .get_client()
            .post(endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("RPC erro pra {}: {}", address, e))?;

        if !resp.status().is_success() {
            return Err(format!(
                "RPC status {} pra {}",
                resp.status(),
                address
            ));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("RPC parse pra {}: {}", address, e))?;

        if let Some(err) = body.get("error") {
            return Err(format!("RPC error pra {}: {}", address, err));
        }

        body.get("result")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("RPC sem `result` pra {}", address))
    }

    /// Lista de opcodes (apenas o nome, sem operandos PUSH) do bytecode.
    /// Usa o disassembler já existente em `utils::my_disassembler`.
    fn opcode_skeleton(clean_bytecode: &str) -> Vec<String> {
        let bytes = match hex::decode(clean_bytecode) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        let bc: revmasm::types::bytecodes::Bytecodes = bytes.into();
        crate::utils::my_disassembler::my_disassemble(bc)
            .into_iter()
            .map(|i| i.name)
            .collect()
    }

    /// Jaccard sobre 4-shingles de opcodes. Captura padrões locais melhor
    /// do que comparar sets simples (que perdem ordem) ou subsequências
    /// longas (caras de calcular). 4-grams é o sweet spot pra EVM.
    fn opcode_skeleton_jaccard(clean_a: &str, clean_b: &str) -> f64 {
        let ops_a = Self::opcode_skeleton(clean_a);
        let ops_b = Self::opcode_skeleton(clean_b);

        if ops_a.is_empty() && ops_b.is_empty() {
            return 1.0;
        }

        // Bytecodes muito curtos (<4 opcodes): cai pro Jaccard de set simples.
        if ops_a.len() < 4 || ops_b.len() < 4 {
            let set_a: std::collections::HashSet<&str> = ops_a.iter().map(|s| s.as_str()).collect();
            let set_b: std::collections::HashSet<&str> = ops_b.iter().map(|s| s.as_str()).collect();
            let shared = set_a.intersection(&set_b).count();
            let union = set_a.union(&set_b).count();
            return if union == 0 { 1.0 } else { shared as f64 / union as f64 };
        }

        let shingles_a: std::collections::HashSet<String> = (0..=ops_a.len() - 4)
            .map(|i| ops_a[i..i + 4].join(","))
            .collect();
        let shingles_b: std::collections::HashSet<String> = (0..=ops_b.len() - 4)
            .map(|i| ops_b[i..i + 4].join(","))
            .collect();

        let shared = shingles_a.intersection(&shingles_b).count();
        let union = shingles_a.union(&shingles_b).count();
        if union == 0 {
            1.0
        } else {
            shared as f64 / union as f64
        }
    }

    pub async fn enrich(
        &self,
        provider: Arc<Provider<Ws>>,
        payload: &FindDeploysPayload,
    ) -> EnrichedDeploy {
        // Small delay to let the contract propagate on the RPC
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let addr = payload.contract_address;
        let addr_str = format!("{:?}", addr);
        let deployer = payload.from;
        let deployer_str = format!("{:?}", deployer);

        // First batch: token info + wallet info (most critical)
        let (token_info, wallet_info) = tokio::join!(
            self.fetch_token_info(provider.clone(), addr),
            self.fetch_wallet_info(provider.clone(), deployer),
        );

        // Second batch: fees, owner, limits, pair, verified
        let (fee_info, owner_info, verified_info, pair_info, limits_info) = tokio::join!(
            self.fetch_fees(provider.clone(), addr),
            self.fetch_owner(provider.clone(), addr),
            self.fetch_verified_status(&addr_str),
            self.fetch_pair_address(provider.clone(), addr),
            self.fetch_limits(provider.clone(), addr),
        );

        let (name, symbol, decimals, total_supply, total_supply_raw) = token_info;
        let (balance, nonce, funding_source, funding_amount, funding_source_full) = wallet_info;
        let (buy_fee, sell_fee) = fee_info;
        let (is_renounced, owner_address) = owner_info;
        let (is_verified, verified_compiler, socials) = verified_info;
        let (max_tx_raw, max_wallet_raw) = limits_info;

        let (max_tx, max_tx_pct) = Self::format_limit(max_tx_raw, total_supply_raw, decimals);
        let (max_wallet, max_wallet_pct) = Self::format_limit(max_wallet_raw, total_supply_raw, decimals);

        let contract_functions = self.extract_functions_from_bytecode(
            provider.clone(), addr, &payload.input, total_supply_raw, decimals,
        ).await;

        let (buy_gas, sell_gas, anvil_buy_fee, anvil_sell_fee) = {
            let anvil_sim_opt = {
                let lock = self.anvil_simulation.read().await;
                lock.clone()
            };
            let rpc_opt = {
                let lock = self.rpc_endpoint.read().await;
                lock.clone()
            };

            let mut result_gas = (None, None, None, None);

            // 1. Try Anvil simulation first
            if let (Some(anvil_sim), Some(rpc)) = (anvil_sim_opt, rpc_opt) {
                if let Some(sim_result) = anvil_sim.simulate(
                    &rpc, addr, deployer, &payload.input,
                    payload.block_number.map(|b| b.as_u64()),
                ).await {
                    let bg = Some(sim_result.buy_gas).filter(|&g| g > 0);
                    let sg = Some(sim_result.sell_gas).filter(|&g| g > 0);
                    result_gas = (
                        bg, sg,
                        Some(format!("{:.2}", sim_result.buy_tax)),
                        Some(format!("{:.2}", sim_result.sell_tax)),
                    );
                }
            }

            // 2. If Anvil didn't give gas, try ONE quick live attempt (matches Legacy behavior).
            // Legacy sends the message immediately if it can't simulate; we do the same.
            if result_gas.0.is_none() || result_gas.1.is_none() {
                let pair = self.fetch_pair_address(provider.clone(), addr).await;
                if let Some(ref pair_addr) = pair {
                    let bg = self.estimate_buy_gas(provider.clone(), addr, Some(pair_addr.as_str())).await;
                    let sg = self.estimate_sell_gas(provider.clone(), addr, Some(pair_addr.as_str())).await;
                    result_gas.0 = result_gas.0.or(bg);
                    result_gas.1 = result_gas.1.or(sg);
                }
            }

            result_gas
        };

        eprintln!(
            "[ENRICH] {} - buy_gas={:?}, sell_gas={:?}, buy_fee={:?}, sell_fee={:?}, anvil_buy_fee={:?}, anvil_sell_fee={:?}",
            addr_str, buy_gas, sell_gas, buy_fee, sell_fee, anvil_buy_fee, anvil_sell_fee
        );

        // Use Anvil fees as fallback when contract selectors don't have fee getters
        let buy_fee = buy_fee.or(anvil_buy_fee);
        let sell_fee = sell_fee.or(anvil_sell_fee);

        // For checksum: use label if exists, else full address (matches Legacy: label || address)
        let funding_for_checksum = match (&funding_source, &funding_source_full) {
            (Some(src), Some(full)) => {
                if src.contains("...") {
                    Some(full.clone())
                } else {
                    Some(src.clone())
                }
            }
            (_, full) => full.clone(),
        };

        let (bytecode_checksums, function_checksums) = self.build_all_checksums(
            &payload.input,
            provider.clone(),
            addr,
            &funding_for_checksum,
            &buy_fee,
            &sell_fee,
            &max_tx,
            total_supply_raw,
            buy_gas,
            sell_gas,
        ).await;

        // Check ignores — if any checksum is in the ignore list, skip notification
        let ignores = self.sqlite_repository.get_ignores().unwrap_or_default();
        let all_checksums: Vec<&str> = bytecode_checksums.iter()
            .chain(function_checksums.iter())
            .map(|e| e.hex.as_str())
            .collect();
        let _is_ignored = all_checksums.iter().any(|ck| ignores.contains(&ck.to_string()));

        // Lookup annotations by bytecode checksum, fallback to functions checksum (like Legacy)
        let bc_ck = bytecode_checksums.first().map(|e| e.hex.as_str()).unwrap_or("");
        let fn_ck = function_checksums.first().map(|e| e.hex.as_str()).unwrap_or("");
        let all_annotations = self.sqlite_repository.get_annotations().unwrap_or_default();
        let all_gas_annotations = self.sqlite_repository.get_gas_annotations().unwrap_or_default();
        let annotation = all_annotations.get(bc_ck).cloned()
            .or_else(|| all_annotations.get(fn_ck).cloned());
        let gas_annotation = all_gas_annotations.get(bc_ck).cloned()
            .or_else(|| all_gas_annotations.get(fn_ck).cloned());

        EnrichedDeploy {
            contract_address: addr_str,
            name,
            symbol,
            decimals,
            total_supply,
            total_supply_raw,
            buy_fee,
            sell_fee,
            max_tx,
            max_tx_pct,
            max_wallet,
            max_wallet_pct,
            deployer: deployer_str,
            deployer_balance: format_ether(balance),
            deployer_nonce: nonce.to_string(),
            funding_source,
            funding_amount,
            buy_gas,
            sell_gas,
            bytecode_checksums,
            function_checksums,
            contract_functions,
            is_renounced,
            owner_address,
            is_verified,
            verified_compiler,
            socials,
            block_number: payload.block_number,
            pair_address: pair_info,
            pair_buy_gas: None,
            pair_sell_gas: None,
            honeypot_result: None,
            annotation,
            gas_annotation,
            funding_source_full,
            is_scam: false,
        }
    }

    async fn call_selector(
        provider: Arc<Provider<Ws>>,
        address: H160,
        selector: &str,
    ) -> Option<Bytes> {
        let data = hex::decode(selector).ok()?;
        for attempt in 0..3 {
            let tx = TransactionRequest::new()
                .to(address)
                .data(Bytes::from(data.clone()));
            let typed: TypedTransaction = tx.into();
            match provider.call(&typed, None).await {
                Ok(result) => return Some(result),
                Err(_) if attempt < 2 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(200 * (attempt + 1) as u64)).await;
                }
                Err(_) => return None,
            }
        }
        None
    }

    fn decode_string(bytes: &Bytes) -> Option<String> {
        decode(&[ParamType::String], bytes)
            .ok()
            .and_then(|t| t.into_iter().next())
            .and_then(|t| t.into_string())
    }

    fn decode_uint(bytes: &Bytes) -> Option<U256> {
        decode(&[ParamType::Uint(256)], bytes)
            .ok()
            .and_then(|t| t.into_iter().next())
            .and_then(|t| t.into_uint())
    }

    fn decode_address(bytes: &Bytes) -> Option<H160> {
        decode(&[ParamType::Address], bytes)
            .ok()
            .and_then(|t| t.into_iter().next())
            .and_then(|t| t.into_address())
    }

    fn decode_uint8(bytes: &Bytes) -> Option<u8> {
        Self::decode_uint(bytes).map(|u| u.as_u32() as u8)
    }

    async fn fetch_token_info(
        &self,
        provider: Arc<Provider<Ws>>,
        address: H160,
    ) -> (Option<String>, Option<String>, Option<u8>, Option<String>, Option<U256>) {
        let (name_res, symbol_res, decimals_res, supply_res) = tokio::join!(
            Self::call_selector(provider.clone(), address, SELECTOR_NAME),
            Self::call_selector(provider.clone(), address, SELECTOR_SYMBOL),
            Self::call_selector(provider.clone(), address, SELECTOR_DECIMALS),
            Self::call_selector(provider.clone(), address, SELECTOR_TOTAL_SUPPLY),
        );

        let name = name_res.as_ref().and_then(Self::decode_string);
        let symbol = symbol_res.as_ref().and_then(Self::decode_string);
        let decimals = decimals_res.as_ref().and_then(Self::decode_uint8);
        let total_supply = supply_res.as_ref().and_then(Self::decode_uint);

        let formatted_supply = match (total_supply, decimals) {
            (Some(supply), Some(dec)) => {
                let divisor = U256::exp10(dec as usize);
                if divisor.is_zero() {
                    Some(supply.to_string())
                } else {
                    let whole = supply / divisor;
                    Some(Self::format_number_with_dots(&whole.to_string()))
                }
            }
            (Some(supply), None) => Some(supply.to_string()),
            _ => None,
        };

        (name, symbol, decimals, formatted_supply, total_supply)
    }

    fn format_number_with_dots(num: &str) -> String {
        let chars: Vec<char> = num.chars().rev().collect();
        let mut result = String::new();
        for (i, c) in chars.iter().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push('.');
            }
            result.push(*c);
        }
        result.chars().rev().collect()
    }

    async fn fetch_wallet_info(
        &self,
        provider: Arc<Provider<Ws>>,
        deployer: H160,
    ) -> (U256, U256, Option<String>, Option<String>, Option<String>) {
        let (balance, nonce) = tokio::join!(
            provider.get_balance(NameOrAddress::Address(deployer), None),
            provider.get_transaction_count(NameOrAddress::Address(deployer), None),
        );

        let balance = balance.unwrap_or(U256::zero());
        let nonce = nonce.unwrap_or(U256::zero());

        let (funding_source, funding_amount, funding_source_full) =
            self.trace_funding(provider.clone(), deployer).await;

        (balance, nonce, funding_source, funding_amount, funding_source_full)
    }

    async fn trace_funding(
        &self,
        provider: Arc<Provider<Ws>>,
        deployer: H160,
    ) -> (Option<String>, Option<String>, Option<String>) {
        let deployer_str = format!("{:?}", deployer);

        let api_key = self.etherscan_api_key.read().await;
        if let Some(ref key) = *api_key {
            let result = self.trace_funding_etherscan(&deployer_str, key).await;
            if result.0.is_some() {
                return result;
            }
        }
        drop(api_key);

        self.trace_funding_blocks(provider, deployer).await
    }

    async fn trace_funding_etherscan(
        &self,
        deployer: &str,
        api_key: &str,
    ) -> (Option<String>, Option<String>, Option<String>) {
        let client = HttpClient::new();

        // Etherscan v2 normalmente leva 5–30s para indexar uma tx fresca.
        // Quando o bot pinga um deploy logo após acontecer, a primeira
        // request volta "No transactions found" — se desistirmos rápido,
        // o fallback `trace_funding_blocks` também falha (a única tx do
        // deployer é o próprio CREATE, que tem ele como `from`, não como
        // `to`), e a linha de Funding some da mensagem.
        //
        // Damos até ~30s no total: 8 tentativas × 4s. O enrichment roda
        // em uma task por-deploy, então não bloqueia outros pings.
        const ATTEMPTS: u32 = 8;
        const RETRY_DELAY_MS: u64 = 4_000;

        for attempt in 0..ATTEMPTS {
            let url = format!(
                "https://api.etherscan.io/v2/api?chainid=1&module=account&action=txlist&address={}&startblock=0&endblock=99999999&page=1&offset=1&sort=asc&apikey={}",
                deployer, api_key
            );

            let resp = match client.get_client().get(&url).send().await {
                Ok(r) => r,
                Err(_) => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
            };

            let body: serde_json::Value = match resp.json().await {
                Ok(b) => b,
                Err(_) => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
            };

            let message = body.get("message").and_then(|m| m.as_str()).unwrap_or("");
            if message == "No transactions found" {
                tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                continue;
            }

            let result = body.get("result").and_then(|r| r.as_array());
            if let Some(arr) = result {
                if let Some(first) = arr.first() {
                    let from = first.get("from").and_then(|f| f.as_str()).unwrap_or("");
                    let value_wei = first.get("value").and_then(|v| v.as_str()).unwrap_or("0");

                    let from_str = from.to_string();
                    let short = format!(
                        "0x{}...{}",
                        &from_str[2..6.min(from_str.len())],
                        &from_str[from_str.len().saturating_sub(3)..]
                    );

                    let value_u256 = U256::from_dec_str(value_wei).unwrap_or(U256::zero());
                    let amount = format_ether(value_u256);

                    let label = self.sqlite_repository
                        .search_labels_by_address(&from_str)
                        .ok()
                        .and_then(|labels| labels.first().map(|l| l.label.clone()));

                    let display = label.unwrap_or(short);

                    eprintln!(
                        "[FUNDING] etherscan ok após {} tentativa(s) para {}: {} ETH from {}",
                        attempt + 1, deployer, amount, from_str,
                    );
                    return (Some(display), Some(amount), Some(from_str));
                }
            }

            // Resposta válida mas sem result utilizável → não vale a pena
            // continuar tentando.
            break;
        }

        eprintln!(
            "[FUNDING] etherscan esgotou {} tentativas para {} (sem funding visível)",
            ATTEMPTS, deployer,
        );
        (None, None, None)
    }

    async fn trace_funding_blocks(
        &self,
        provider: Arc<Provider<Ws>>,
        deployer: H160,
    ) -> (Option<String>, Option<String>, Option<String>) {
        let latest = match provider.get_block_number().await {
            Ok(n) => n.as_u64(),
            Err(_) => return (None, None, None),
        };

        let start = if latest > 100 { latest - 100 } else { 0 };

        for block_num in (start..=latest).rev() {
            let block_id = BlockId::Number(BlockNumber::Number(block_num.into()));
            let block = match provider.get_block_with_txs(block_id).await {
                Ok(Some(b)) => b,
                _ => continue,
            };

            for tx in block.transactions {
                if tx.to == Some(deployer) && tx.value > U256::zero() {
                    let from_str = format!("{:?}", tx.from);
                    let short = format!(
                        "0x{}...{}",
                        &from_str[2..6],
                        &from_str[from_str.len() - 3..]
                    );
                    return (Some(short), Some(format_ether(tx.value)), Some(from_str));
                }
            }
        }

        (None, None, None)
    }

    async fn fetch_fees(
        &self,
        provider: Arc<Provider<Ws>>,
        address: H160,
    ) -> (Option<String>, Option<String>) {
        let buy_selectors = [
            "4f7041a5", "0f3a325f", "a8aa1b31",
            "f088d547", "f5648a4f", "bfd79284",
            "6d8aa8f8", "56e6c614",
        ];
        let sell_selectors = [
            "b0bc85de", "7bce5a04", "d6e242b8",
            "5b65b9ab", "4f323db3", "1c97a387",
            "d5914dc0", "e0f04e59",
        ];

        let (buy_fee, sell_fee) = tokio::join!(
            self.try_fee_selectors(provider.clone(), address, &buy_selectors),
            self.try_fee_selectors(provider.clone(), address, &sell_selectors),
        );

        (buy_fee, sell_fee)
    }

    async fn try_fee_selectors(
        &self,
        provider: Arc<Provider<Ws>>,
        address: H160,
        selectors: &[&str],
    ) -> Option<String> {
        for selector in selectors {
            if let Some(bytes) = Self::call_selector(provider.clone(), address, selector).await {
                if let Some(val) = Self::decode_uint(&bytes) {
                    if val <= U256::from(10000u64) {
                        let pct = val.as_u64() as f64 / 100.0;
                        return Some(format!("{:.2}", pct));
                    }
                }
            }
        }
        None
    }

    async fn fetch_limits(
        &self,
        provider: Arc<Provider<Ws>>,
        address: H160,
    ) -> (Option<U256>, Option<U256>) {
        let max_tx_selectors = ["8f9a55c0", "e8078d94", "3582ad23", "cf188ad0"];
        let max_wallet_selectors = ["17e1df56", "1a8145bb", "59927044", "106a0535"];

        let (max_tx, max_wallet) = tokio::join!(
            self.try_limit_selectors(provider.clone(), address, &max_tx_selectors),
            self.try_limit_selectors(provider.clone(), address, &max_wallet_selectors),
        );

        (max_tx, max_wallet)
    }

    async fn try_limit_selectors(
        &self,
        provider: Arc<Provider<Ws>>,
        address: H160,
        selectors: &[&str],
    ) -> Option<U256> {
        for selector in selectors {
            if let Some(bytes) = Self::call_selector(provider.clone(), address, selector).await {
                if let Some(val) = Self::decode_uint(&bytes) {
                    if val > U256::zero() {
                        return Some(val);
                    }
                }
            }
        }
        None
    }

    fn format_limit(
        raw: Option<U256>,
        total_supply: Option<U256>,
        decimals: Option<u8>,
    ) -> (Option<String>, Option<String>) {
        let val = match raw {
            Some(v) => v,
            None => return (None, None),
        };

        let formatted = match decimals {
            Some(dec) => {
                let divisor = U256::exp10(dec as usize);
                if divisor.is_zero() {
                    Self::format_number_with_dots(&val.to_string())
                } else {
                    Self::format_number_with_dots(&(val / divisor).to_string())
                }
            }
            None => Self::format_number_with_dots(&val.to_string()),
        };

        let pct = match total_supply {
            Some(supply) if supply > U256::zero() => {
                let pct_val = val.checked_mul(U256::from(10000u64))
                    .unwrap_or(U256::MAX) / supply;
                let pct_u = if pct_val > U256::from(u64::MAX) { u64::MAX } else { pct_val.as_u64() };
                let pct_f = pct_u as f64 / 100.0;
                Some(format!("{:.2}%", pct_f))
            }
            _ => None,
        };

        (Some(formatted), pct)
    }

    async fn estimate_buy_gas(
        &self,
        provider: Arc<Provider<Ws>>,
        token: H160,
        pair: Option<&str>,
    ) -> Option<u64> {
        if pair.is_none() {
            return None;
        }
        let router: H160 = "7a250d5630B4cF539739dF2C5dAcb4c659F2488D".parse().ok()?;
        let weth: H160 = "C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".parse().ok()?;
        let dead: H160 = "000000000000000000000000000000000000dEaD".parse().ok()?;

        let deadline = U256::from(u64::MAX);
        let amount_in = ethers::utils::parse_ether("0.01").ok()?;
        let mut calldata = hex::decode("7ff36ab5").ok()?;
        calldata.extend_from_slice(&[0u8; 32]); // amountOutMin = 0
        calldata.extend_from_slice(&ethers::abi::encode(&[
            ethers::abi::Token::Uint(U256::from(128u64)),
        ]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(dead.as_bytes()); // to = dead address (receiver)
        calldata.extend_from_slice(&ethers::abi::encode(&[
            ethers::abi::Token::Uint(deadline),
        ]));
        calldata.extend_from_slice(&ethers::abi::encode(&[
            ethers::abi::Token::Uint(U256::from(2u64)),
        ]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(weth.as_bytes());
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token.as_bytes());

        let tx = TransactionRequest::new()
            .to(router)
            .data(Bytes::from(calldata))
            .value(amount_in);
        let typed: TypedTransaction = tx.into();

        match provider.estimate_gas(&typed, None).await {
            Ok(gas) => {
                let g = if gas > U256::from(u64::MAX) { u64::MAX } else { gas.as_u64() };
                Some(g)
            }
            Err(e) => {
                eprintln!("[ENRICH] buy gas estimate failed: {}", e);
                None
            }
        }
    }

    async fn estimate_sell_gas(
        &self,
        provider: Arc<Provider<Ws>>,
        token: H160,
        pair: Option<&str>,
    ) -> Option<u64> {
        if pair.is_none() {
            return None;
        }
        let router: H160 = "7a250d5630B4cF539739dF2C5dAcb4c659F2488D".parse().ok()?;
        let weth: H160 = "C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".parse().ok()?;

        let deadline = U256::from(u64::MAX);
        let amount_in = U256::from(1000u64);
        // swapExactTokensForETH (standard, matches Legacy)
        let mut calldata = hex::decode("18cbafe5").ok()?;
        calldata.extend_from_slice(&ethers::abi::encode(&[
            ethers::abi::Token::Uint(amount_in),
        ]));
        calldata.extend_from_slice(&[0u8; 32]); // amountOutMin = 0
        calldata.extend_from_slice(&ethers::abi::encode(&[
            ethers::abi::Token::Uint(U256::from(160u64)),
        ]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token.as_bytes());
        calldata.extend_from_slice(&ethers::abi::encode(&[
            ethers::abi::Token::Uint(deadline),
        ]));
        calldata.extend_from_slice(&ethers::abi::encode(&[
            ethers::abi::Token::Uint(U256::from(2u64)),
        ]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token.as_bytes());
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(weth.as_bytes());

        let tx = TransactionRequest::new()
            .to(router)
            .data(Bytes::from(calldata));
        let typed: TypedTransaction = tx.into();

        match provider.estimate_gas(&typed, None).await {
            Ok(gas) => {
                let g = if gas > U256::from(u64::MAX) { u64::MAX } else { gas.as_u64() };
                Some(g)
            }
            Err(e) => {
                eprintln!("[ENRICH] sell gas estimate failed: {}", e);
                None
            }
        }
    }

    async fn lookup_4byte_signature(&self, hex_id: &str) -> Option<String> {
        let url = format!(
            "https://www.4byte.directory/api/v1/signatures/?hex_signature=0x{}",
            hex_id
        );
        let client = HttpClient::new();
        let resp = match client.get_client().get(&url).send().await {
            Ok(r) => r,
            Err(_) => return None,
        };
        let body: serde_json::Value = match resp.json().await {
            Ok(b) => b,
            Err(_) => return None,
        };
        let results = body.get("results")?.as_array()?;
        if results.is_empty() {
            return None;
        }

        // Pick the shortest signature to avoid spam/false positives
        // (e.g. "niceFunctionHerePlzClick943230089" vs "setApprovalForAll")
        let best = results
            .iter()
            .filter_map(|r| r.get("text_signature")?.as_str())
            .min_by_key(|s| s.len())?;

        let _ = self.sqlite_repository.upsert_signature(
            hex_id,
            best,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        );

        Some(best.to_string())
    }

    async fn extract_functions_from_bytecode(
        &self,
        provider: Arc<Provider<Ws>>,
        address: H160,
        bytecode: &str,
        total_supply_raw: Option<U256>,
        decimals: Option<u8>,
    ) -> Vec<ContractFunction> {
        let clean = bytecode.replace("0x", "");
        let bytes = match hex::decode(&clean) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };

        // Use DUP1+PUSH4 (8063) pattern like Legacy for accurate selector extraction
        let mut selectors: Vec<String> = Vec::new();
        let mut i = 0;
        while i + 6 < bytes.len() {
            if bytes[i] == 0x80 && bytes[i + 1] == 0x63 {
                let selector = format!(
                    "0x{:02x}{:02x}{:02x}{:02x}",
                    bytes[i + 2], bytes[i + 3], bytes[i + 4], bytes[i + 5]
                );
                if !selectors.contains(&selector) {
                    selectors.push(selector);
                }
                i += 6;
            } else {
                i += 1;
            }
        }
        // Fallback to PUSH4-only if no selectors found (like Legacy)
        if selectors.is_empty() {
            let mut i = 0;
            while i + 5 < bytes.len() {
                if bytes[i] == 0x63 {
                    let selector = format!(
                        "0x{:02x}{:02x}{:02x}{:02x}",
                        bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]
                    );
                    if !selectors.contains(&selector) {
                        selectors.push(selector);
                    }
                    i += 5;
                } else {
                    i += 1;
                }
            }
        }

        let hex_ids: Vec<String> = selectors.iter().map(|s| s.replace("0x", "")).collect();
        let sig_map: HashMap<String, String> = self
            .sqlite_repository
            .get_signatures_by_ids(&hex_ids)
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.hex_signature.to_lowercase(), s.text_signature))
            .collect();

        // Matches Legacy's SignaturesUtil.ignoreSignatures exactly
        let standard_selectors = STANDARD_ERC20_SELECTORS;

        // Load indicators/tags for individual selectors (like Legacy)
        let indicators: std::collections::HashMap<String, String> = self
            .sqlite_repository
            .get_indicators()
            .unwrap_or_default();

        let mut results = Vec::new();

        for sel in selectors {
            let hex_key = sel.replace("0x", "").to_lowercase();

            if standard_selectors.contains(&hex_key.as_str()) {
                continue;
            }

            let name = match sig_map.get(&hex_key).cloned() {
                Some(n) => Some(n),
                None => self.lookup_4byte_signature(&hex_key).await,
            };

            let (return_value, return_pct) =
                self.try_call_function(provider.clone(), address, &hex_key, total_supply_raw, decimals).await;

            let tag = indicators.get(&sel).cloned()
                .or_else(|| indicators.get(&format!("0x{}", hex_key)).cloned())
                .or_else(|| indicators.get(&hex_key).cloned());

            results.push(ContractFunction {
                selector: sel,
                name,
                return_value,
                return_pct,
                tag,
            });
        }

        results
    }

    async fn try_call_function(
        &self,
        provider: Arc<Provider<Ws>>,
        address: H160,
        selector: &str,
        total_supply_raw: Option<U256>,
        _decimals: Option<u8>,
    ) -> (Option<String>, Option<String>) {
        let bytes = match Self::call_selector(provider, address, selector).await {
            Some(b) => b,
            None => return (None, None),
        };

        if bytes.len() != 32 {
            return (None, None);
        }

        let hex_bytes = format!("0x{}", hex::encode(&bytes));

        // Boolean detection (matches Legacy: exact 32-byte check)
        if hex_bytes == "0x0000000000000000000000000000000000000000000000000000000000000000" {
            return (Some("false".to_string()), None);
        }
        if hex_bytes == "0x0000000000000000000000000000000000000000000000000000000000000001" {
            return (Some("true".to_string()), None);
        }

        let val = match Self::decode_uint(&bytes) {
            Some(v) => v,
            None => return (None, None),
        };

        // Address detection (matches Legacy: if value string > 42 chars, extract address)
        let raw_str = val.to_string();
        if raw_str.len() > 42 {
            let addr_hex = format!("0x{}", &hex::encode(&bytes)[24..]);
            if addr_hex.len() == 42 {
                return (None, None);
            }
        }

        if val.is_zero() {
            return (None, None);
        }

        let pct = match total_supply_raw {
            Some(supply) if supply > U256::zero() => {
                let pct_val = val.checked_mul(U256::from(10000u64))
                    .unwrap_or(U256::MAX) / supply;
                let pct_u = if pct_val > U256::from(u64::MAX) { u64::MAX } else { pct_val.as_u64() };
                let pct_f = pct_u as f64 / 100.0;
                Some(format!("{:.2}%", pct_f))
            }
            _ => None,
        };

        (Some(raw_str), pct)
    }

    async fn fetch_owner(
        &self,
        provider: Arc<Provider<Ws>>,
        address: H160,
    ) -> (bool, Option<String>) {
        if let Some(bytes) = Self::call_selector(provider, address, SELECTOR_OWNER).await {
            if let Some(owner) = Self::decode_address(&bytes) {
                let is_renounced = owner == H160::zero();
                let owner_str = format!("{:?}", owner);
                return (is_renounced, Some(owner_str));
            }
        }
        (false, None)
    }

    async fn fetch_pair_address(
        &self,
        provider: Arc<Provider<Ws>>,
        token: H160,
    ) -> Option<String> {
        let factory: H160 = "5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f".parse().ok()?;
        let weth: H160 = "C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".parse().ok()?;

        let mut calldata = hex::decode("e6a43905").ok()?;
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token.as_bytes());
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(weth.as_bytes());

        let tx = TransactionRequest::new()
            .to(factory)
            .data(Bytes::from(calldata));
        let typed: TypedTransaction = tx.into();
        let result = provider.call(&typed, None).await.ok()?;

        let pair = Self::decode_address(&result)?;
        if pair == H160::zero() {
            return None;
        }
        Some(format!("{:?}", pair))
    }

    async fn build_all_checksums(
        &self,
        bytecode: &str,
        provider: Arc<Provider<Ws>>,
        address: H160,
        funding_source: &Option<String>,
        buy_fee: &Option<String>,
        sell_fee: &Option<String>,
        max_tx: &Option<String>,
        total_supply_raw: Option<U256>,
        buy_gas: Option<u64>,
        sell_gas: Option<u64>,
    ) -> (Vec<ChecksumEntry>, Vec<ChecksumEntry>) {
        let mut bc_entries = Vec::new();
        let mut fn_entries = Vec::new();

        let clean = bytecode.replace("0x", "");
        let selector_list = Self::extract_selectors_from_bytecode(&clean);
        let selector_strs: Vec<&str> = selector_list.iter().map(|s| s.as_str()).collect();

        eprintln!("[CHECKSUM] {:?} - {} selectors extracted: {:?}", address, selector_list.len(), selector_list);

        // Fetch RUNTIME bytecode for Heimdall CFG checksum (matches Legacy algorithm)
        let runtime_code = match provider.get_code(address, None).await {
            Ok(code) => {
                let hex_code = format!("0x{}", hex::encode(&code));
                eprintln!("[CHECKSUM] Got runtime code: {} bytes", code.len());
                hex_code
            }
            Err(e) => {
                eprintln!("[CHECKSUM] Failed to get runtime code: {}, using deployment bytecode", e);
                bytecode.to_string()
            }
        };

        // Bytecode checksum via Heimdall CFG opcodes on RUNTIME code (same algorithm as Legacy)
        let bc_checksum = Self::checksum_by_opcode(&runtime_code).await;
        bc_entries.push(self.make_checksum_entry("Bytecode", &bc_checksum, false));

        let fn_checksum = Self::composed_keccak256(&selector_strs);
        fn_entries.push(self.make_checksum_entry("Functions", &fn_checksum, false));

        eprintln!("[CHECKSUM] {:?} - Bytecode={} Functions={}", address, bc_checksum, fn_checksum);

        // --- Bytecode children ---
        if let Some(supply) = total_supply_raw {
            let supply_str = supply.to_string();
            let hash = Self::composed_keccak256(&[&bc_checksum, &supply_str]);
            bc_entries.push(self.make_checksum_entry("Total Supply", &hash, true));
        }

        if let Some(ref src) = funding_source {
            let hash = Self::composed_keccak256(&[&bc_checksum, src.as_str()]);
            bc_entries.push(self.make_checksum_entry("Funding", &hash, true));
        }

        if let Some(bg) = buy_gas {
            let gas_str = bg.to_string();
            let hash = Self::composed_keccak256(&[&bc_checksum, &gas_str]);
            bc_entries.push(self.make_checksum_entry("Buy Gas", &hash, true));
        }

        if let Some(sg) = sell_gas {
            let gas_str = sg.to_string();
            let hash = Self::composed_keccak256(&[&bc_checksum, &gas_str]);
            bc_entries.push(self.make_checksum_entry("Sell Gas", &hash, true));
        }

        if let Some(ref fee) = buy_fee {
            let hash = Self::composed_keccak256(&[&bc_checksum, fee.as_str()]);
            bc_entries.push(self.make_checksum_entry("Buy Fee", &hash, true));
        }

        if let Some(ref fee) = sell_fee {
            let hash = Self::composed_keccak256(&[&bc_checksum, fee.as_str()]);
            bc_entries.push(self.make_checksum_entry("Sell Fee", &hash, true));
        }

        if let Some(ref mx) = max_tx {
            let hash = Self::composed_keccak256(&[&bc_checksum, mx.as_str()]);
            bc_entries.push(self.make_checksum_entry("Max TX", &hash, true));
        }

        // --- Functions children ---
        if let Some(supply) = total_supply_raw {
            let supply_str = supply.to_string();
            let hash = Self::composed_keccak256(&[&fn_checksum, &supply_str]);
            fn_entries.push(self.make_checksum_entry("Total Supply", &hash, true));
        }

        if let Some(ref src) = funding_source {
            let hash = Self::composed_keccak256(&[&fn_checksum, src.as_str()]);
            fn_entries.push(self.make_checksum_entry("Funding", &hash, true));
        }

        if let Some(bg) = buy_gas {
            let gas_str = bg.to_string();
            let hash = Self::composed_keccak256(&[&fn_checksum, &gas_str]);
            fn_entries.push(self.make_checksum_entry("Buy Gas", &hash, true));
        }

        if let Some(sg) = sell_gas {
            let gas_str = sg.to_string();
            let hash = Self::composed_keccak256(&[&fn_checksum, &gas_str]);
            fn_entries.push(self.make_checksum_entry("Sell Gas", &hash, true));
        }

        if let Some(ref fee) = buy_fee {
            let hash = Self::composed_keccak256(&[&fn_checksum, fee.as_str()]);
            fn_entries.push(self.make_checksum_entry("Buy Fee", &hash, true));
        }

        if let Some(ref fee) = sell_fee {
            let hash = Self::composed_keccak256(&[&fn_checksum, fee.as_str()]);
            fn_entries.push(self.make_checksum_entry("Sell Fee", &hash, true));
        }

        if let Some(ref mx) = max_tx {
            let hash = Self::composed_keccak256(&[&fn_checksum, mx.as_str()]);
            fn_entries.push(self.make_checksum_entry("Max TX", &hash, true));
        }

        // Increment counts for all checksums seen in this deploy
        for entry in bc_entries.iter().chain(fn_entries.iter()) {
            let _ = self.sqlite_repository.increment_checksum_history(&entry.hex);
        }

        (bc_entries, fn_entries)
    }

    fn extract_selectors_from_bytecode(clean_bytecode: &str) -> Vec<String> {
        let bytes = match hex::decode(clean_bytecode) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };

        let mut selectors = Vec::new();
        let mut seen = std::collections::HashSet::new();

        let mut i = 0;
        while i + 5 < bytes.len() {
            if i > 0 && bytes[i - 1] == 0x80 && bytes[i] == 0x63 {
                let sel = format!(
                    "0x{:02x}{:02x}{:02x}{:02x}",
                    bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]
                );
                if seen.insert(sel.clone()) {
                    selectors.push(sel);
                }
                i += 5;
            } else {
                i += 1;
            }
        }

        if selectors.is_empty() {
            i = 0;
            while i + 5 < bytes.len() {
                if bytes[i] == 0x63 {
                    let sel = format!(
                        "0x{:02x}{:02x}{:02x}{:02x}",
                        bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]
                    );
                    if seen.insert(sel.clone()) {
                        selectors.push(sel);
                    }
                    i += 5;
                } else {
                    i += 1;
                }
            }
        }

        selectors
    }

    async fn checksum_by_opcode(bytecode: &str) -> String {
        let heimdall = HeimdallService {};
        let blocks = match heimdall.get_cfg_as_json(bytecode.to_string()).await {
            Ok(b) => b,
            Err(_) => {
                eprintln!("[CHECKSUM] Heimdall CFG failed, falling back to raw bytecode hash");
                let clean = bytecode.replace("0x", "");
                return Self::composed_keccak256(&[&clean]);
            }
        };

        // Opcode equivalence mapping (same as Legacy)
        let opcode_equivalents: HashMap<&str, &str> = [
            ("SELFBALANCE", "BALANCE"),
            ("STATICCALL", "CALL"),
            ("MSTORE8", "MSTORE"),
            ("EXTCODESIZE", "EXTCODEHASH"),
        ].iter().cloned().collect();

        // filterPureBusinessLogic (exact Legacy algorithm)
        let filtered: Vec<&Vec<crate::services::heimdall_service::Weight>> = blocks.iter().filter(|block| {
            let ops: Vec<&str> = block.iter().map(|w| w.op.as_str()).collect();

            // Exclude blocks with proxy/setup opcodes
            if ops.iter().any(|op| *op == "CALLDATACOPY" || *op == "DELEGATECALL" || *op == "CODECOPY") {
                return false;
            }

            // Must have at least one business logic opcode
            if !ops.iter().any(|op| *op == "SSTORE" || *op == "CALL" || *op == "STATICCALL" || *op == "MUL" || *op == "DIV") {
                return false;
            }

            // Must have JUMPDEST and must not have EXTCODECOPY
            ops.iter().any(|op| *op == "JUMPDEST") && !ops.iter().any(|op| *op == "EXTCODECOPY")
        }).collect();

        // Extract opcodes with equivalence mapping
        let opcodes: Vec<String> = filtered.into_iter().flat_map(|block| {
            block.iter().map(|w| {
                opcode_equivalents.get(w.op.as_str())
                    .map(|eq| eq.to_string())
                    .unwrap_or_else(|| w.op.clone())
            })
        }).collect();

        if opcodes.is_empty() {
            eprintln!("[CHECKSUM] No opcodes after filter, falling back to raw bytecode hash");
            let clean = bytecode.replace("0x", "");
            return Self::composed_keccak256(&[&clean]);
        }

        // composedKeccak256ToGetID with opcodes
        let opcode_strs: Vec<&str> = opcodes.iter().map(|s| s.as_str()).collect();
        Self::composed_keccak256(&opcode_strs)
    }

    fn composed_keccak256(items: &[&str]) -> String {
        let zero_hash = "0x0000000000000000000000000000000000000000000000000000000000000000";
        let mut current_hash = zero_hash.to_string();

        for item in items {
            let concatenated = format!("{}{}", item, current_hash);
            let hash = keccak256(concatenated.as_bytes());
            current_hash = format!("0x{}", hex::encode(hash));
        }

        current_hash[..10].to_string()
    }

    fn make_checksum_entry(&self, label: &str, hex_val: &str, is_sub: bool) -> ChecksumEntry {
        let (scam, total) = self
            .sqlite_repository
            .get_checksum_history(hex_val)
            .unwrap_or((0, 0));
        let pct = if total > 0 {
            format!("{}%", (scam * 100) / total)
        } else {
            "0%".to_string()
        };

        let tag = self.get_checksum_tag(hex_val);

        ChecksumEntry {
            label: label.to_string(),
            hex: hex_val.to_string(),
            scam_count: scam,
            total_count: total,
            percentage: pct,
            is_sub,
            tag,
        }
    }

    fn get_checksum_tag(&self, hex_val: &str) -> Option<String> {
        // Check indicators table first (user-defined tags via /add command)
        if let Ok(indicators) = self.sqlite_repository.get_indicators() {
            if let Some(tag) = indicators.get(hex_val) {
                return Some(tag.clone());
            }
        }
        // Fallback to checksum_contracts extra_fields
        let row = self.sqlite_repository.get_checksum(hex_val).ok()??;
        let extra: serde_json::Value = serde_json::from_str(&row.extra_fields).ok()?;
        extra.get("tag")
            .or_else(|| extra.get("name"))
            .or_else(|| extra.get("label"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Consulta `getsourcecode` do Etherscan e devolve, em uma única chamada:
    ///   - `is_verified` (`Some(true/false)` quando a chamada teve sucesso)
    ///   - `compiler_version` (ex.: `v0.8.20+commit.a1b79de6`)
    ///   - `socials` extraídos do `SourceCode` (twitter/telegram/web/...).
    ///
    /// Espelha o `processVersion` + `processSocials` do Legacy
    /// (`seekers-galaxy/src/populators/Contract.ts`), que reaproveita a mesma
    /// resposta do Etherscan para evitar duas roundtrips.
    async fn fetch_verified_status(
        &self,
        address: &str,
    ) -> (Option<bool>, Option<String>, ContractSocials) {
        let api_key = self.etherscan_api_key.read().await;
        let key = match api_key.as_ref() {
            Some(k) => k.clone(),
            None => return (None, None, ContractSocials::default()),
        };

        let url = format!(
            "https://api.etherscan.io/api?module=contract&action=getsourcecode&address={}&apikey={}",
            address, key
        );

        let client = HttpClient::new();
        let resp = match client.get_client().get(&url).send().await {
            Ok(r) => r,
            Err(_) => return (None, None, ContractSocials::default()),
        };

        let body: serde_json::Value = match resp.json().await {
            Ok(b) => b,
            Err(_) => return (None, None, ContractSocials::default()),
        };

        let result = &body["result"];
        if let Some(arr) = result.as_array() {
            if let Some(first) = arr.first() {
                let compiler = first["CompilerVersion"].as_str().map(|s| s.to_string());
                let abi_str = first["ABI"].as_str().unwrap_or("");
                let is_verified = abi_str != "Contract source code not verified";

                let mut socials = ContractSocials::default();
                if is_verified {
                    self.discover_signatures_from_abi(abi_str);

                    // SourceCode em multi-arquivo vem como JSON serializado
                    // (string que começa com `{{`); single-file é o source cru.
                    // Em ambos os casos um varredor por substring "http(s)://"
                    // sobre o texto bruto recupera os links como o Legacy faz.
                    let source_code = first["SourceCode"].as_str().unwrap_or("");
                    socials = Self::extract_socials_from_source(source_code);
                }

                return (Some(is_verified), compiler, socials);
            }
        }

        (None, None, ContractSocials::default())
    }

    /// Reimplementa `StringUtils.extractLinks` do Legacy
    /// (`seekers-galaxy/src/utils/StringUtils.ts`) sem dependência de regex:
    /// varre a string atrás de `http://` / `https://`, lê até o próximo
    /// whitespace e categoriza pelo host. Mantém apenas o primeiro link de
    /// cada categoria para casar com `links[0]` do TS.
    fn extract_socials_from_source(text: &str) -> ContractSocials {
        let mut socials = ContractSocials::default();
        if text.is_empty() {
            return socials;
        }

        // Etherscan devolve `\n` como literal `\\n` quando o source vem
        // dentro de JSON; normalizamos pra não engolir caracteres do URL
        // adjacente (ex.: `https://x.com/foo\\nhttps://t.me/bar`).
        let normalized = text.replace("\\n", "\n").replace("\\r", "\n");
        let bytes = normalized.as_bytes();
        let mut idx = 0usize;

        while idx < bytes.len() {
            let start = match Self::find_next_url_start(bytes, idx) {
                Some(s) => s,
                None => break,
            };

            let mut end = start;
            while end < bytes.len() {
                let b = bytes[end];
                // `"` e `'` aparecem fechando strings de Solidity/JSON;
                // `<` e `>` em comentários HTML/Markdown. Tratamos como
                // borda do URL pra evitar capturar lixo subsequente.
                if b.is_ascii_whitespace() || b == b'"' || b == b'\'' || b == b'<' || b == b'>' || b == b'`' || b == b'\\' {
                    break;
                }
                end += 1;
            }

            // Limpa pontuação trailing comum em comentários/markdown que não
            // faz parte do URL real.
            let raw = &normalized[start..end];
            let trimmed = raw.trim_end_matches(|c: char| matches!(
                c, ')' | ',' | '.' | ';' | ':' | ']' | '}' | '!' | '?' | '*' | '~'
            ));
            if !trimmed.is_empty() {
                Self::categorize_url(trimmed, &mut socials);
            }
            idx = end.max(start + 1);
        }

        socials
    }

    fn find_next_url_start(bytes: &[u8], from: usize) -> Option<usize> {
        const HTTP: &[u8] = b"http://";
        const HTTPS: &[u8] = b"https://";
        let mut i = from;
        while i < bytes.len() {
            if bytes[i..].starts_with(HTTPS) || bytes[i..].starts_with(HTTP) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn categorize_url(url: &str, socials: &mut ContractSocials) {
        let lower = url.to_lowercase();

        // Mesma lista de hosts ignorados que o Legacy
        // (StringUtils.extractLinks): docs/templates de OpenZeppelin,
        // EIPs, Solidity, Consensys e GitHub não são "social" do projeto.
        if lower.contains("eips.ethereum.org")
            || lower.contains("consensys")
            || lower.contains("solidity")
            || lower.contains("github")
            || lower.contains("zeppelin")
        {
            return;
        }

        let slot: &mut Option<String> = if lower.contains("t.me") || lower.contains("telegram.me") {
            &mut socials.telegram
        } else if lower.contains("x.com") || lower.contains("twitter.com") {
            &mut socials.twitter
        } else if lower.contains("tiktok.com") {
            &mut socials.tiktok
        } else if lower.contains("youtube.com") {
            &mut socials.youtube
        } else if lower.contains("instagram.com") {
            &mut socials.instagram
        } else {
            &mut socials.web
        };

        if slot.is_none() {
            *slot = Some(url.to_string());
        }
    }

    fn discover_signatures_from_abi(&self, abi_str: &str) {
        let abi: Vec<serde_json::Value> = match serde_json::from_str(abi_str) {
            Ok(a) => a,
            Err(_) => return,
        };

        for item in &abi {
            if item["type"].as_str() != Some("function") {
                continue;
            }
            let name = match item["name"].as_str() {
                Some(n) => n,
                None => continue,
            };
            let inputs = item["inputs"].as_array().map(|arr| {
                arr.iter()
                    .filter_map(|i| i["type"].as_str())
                    .collect::<Vec<&str>>()
                    .join(",")
            }).unwrap_or_default();

            let text_sig = format!("{}({})", name, inputs);

            let hash = keccak256(text_sig.as_bytes());
            let hex_sig = format!("{:02x}{:02x}{:02x}{:02x}", hash[0], hash[1], hash[2], hash[3]);

            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let _ = self.sqlite_repository.upsert_signature(&hex_sig, &text_sig, ts);
        }
    }
}
