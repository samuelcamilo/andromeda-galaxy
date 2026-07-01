use crate::http_client::HttpClient;
use crate::repositories::sqlite_repository::SqliteRepository;
use crate::services::enrichment_service::{EnrichedDeploy, EnrichmentService};
use crate::services::ethers::find_deploys::find_deploys_service::FindDeploysPayload;
use ethers::prelude::{Provider, Ws};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::{RwLock, Semaphore};

struct TelegramConfig {
    bot_token: String,
    chat_id: String,
    bot_username: String,
}

struct PendingEnrichment {
    provider: Arc<Provider<Ws>>,
    payload: FindDeploysPayload,
}

pub struct TelegramService {
    config: Arc<RwLock<Option<TelegramConfig>>>,
    sender: mpsc::Sender<PendingEnrichment>,
    enrichment_service: Arc<EnrichmentService>,
    sqlite_repository: Arc<SqliteRepository>,
}

impl TelegramService {
    pub fn new(
        enrichment_service: Arc<EnrichmentService>,
        sqlite_repository: Arc<SqliteRepository>,
    ) -> Self {
        let config: Arc<RwLock<Option<TelegramConfig>>> = Arc::new(RwLock::new(None));
        let (sender, receiver) = mpsc::channel::<PendingEnrichment>(500);

        let worker_config = config.clone();
        let worker_enrichment = enrichment_service.clone();
        let worker_repo = sqlite_repository.clone();

        tokio::spawn(Self::worker_loop(
            worker_config,
            worker_enrichment,
            worker_repo,
            receiver,
        ));

        TelegramService {
            config,
            sender,
            enrichment_service,
            sqlite_repository,
        }
    }

    /// Acesso usado pelo `RugDetectorService` para obter token/chat/username
    /// sem expor a struct privada `TelegramConfig`.
    pub async fn current_config(&self) -> Option<(String, String, String)> {
        let cfg = self.config.read().await;
        cfg.as_ref()
            .map(|c| (c.bot_token.clone(), c.chat_id.clone(), c.bot_username.clone()))
    }

    pub async fn configure(&self, bot_token: String, chat_id: String, etherscan_key: Option<String>, bot_username: Option<String>) {
        *self.config.write().await = Some(TelegramConfig {
            bot_token,
            chat_id,
            bot_username: bot_username.unwrap_or_else(|| "MMChecksumETH_bot".to_string()),
        });

        if let Some(key) = etherscan_key {
            self.enrichment_service.set_etherscan_key(key).await;
        }
    }

    pub async fn is_configured(&self) -> bool {
        self.config.read().await.is_some()
    }

    pub fn notify(&self, provider: Arc<Provider<Ws>>, payload: FindDeploysPayload) {
        if let Err(e) = self.sender.try_send(PendingEnrichment { provider, payload }) {
            eprintln!("[TELEGRAM] Fila de enrichment cheia/fechada, deploy descartado: {}", e);
        }
    }

    pub async fn send_test_message(&self, message: &str) -> Result<(), String> {
        let config = self.config.read().await;
        let cfg = config.as_ref().ok_or("Telegram nao configurado")?;
        Self::send_message(&cfg.bot_token, &cfg.chat_id, message).await?;
        Ok(())
    }

    async fn worker_loop(
        config: Arc<RwLock<Option<TelegramConfig>>>,
        enrichment_service: Arc<EnrichmentService>,
        sqlite_repository: Arc<SqliteRepository>,
        mut receiver: mpsc::Receiver<PendingEnrichment>,
    ) {
        let concurrency = std::env::var("TELEGRAM_ENRICH_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(4);
        let enrich_timeout = Duration::from_secs(
            std::env::var("TELEGRAM_ENRICH_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v >= 30)
                .unwrap_or(180),
        );
        let enrichment_permits = Arc::new(Semaphore::new(concurrency));

        while let Some(pending) = receiver.recv().await {
            let cfg_lock = config.read().await;
            let cfg = match cfg_lock.as_ref() {
                Some(c) => c,
                None => continue,
            };

            let bot_token = cfg.bot_token.clone();
            let chat_id = cfg.chat_id.clone();
            let bot_username = cfg.bot_username.clone();
            drop(cfg_lock);

            let enrichment_service = enrichment_service.clone();
            let repo = sqlite_repository.clone();
            let permit = match enrichment_permits.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => {
                    eprintln!("[TELEGRAM] Semaforo de enrichment fechado");
                    break;
                }
            };

            // Spawn enrichment in parallel, but bounded so Anvil/RPC cannot exhaust the container.
            tokio::spawn(async move {
                let _permit = permit;
                let enrich_result = tokio::time::timeout(
                    enrich_timeout,
                    AssertUnwindSafe(
                        enrichment_service.enrich(pending.provider, &pending.payload),
                    )
                    .catch_unwind(),
                )
                .await;

                let enriched = match enrich_result {
                    Ok(Ok(e)) => e,
                    Ok(Err(_)) => {
                        eprintln!("[TELEGRAM] Panic capturado durante enrich, pulando mensagem");
                        return;
                    }
                    Err(_) => {
                        eprintln!("[TELEGRAM] Timeout de {:?} durante enrich", enrich_timeout);
                        return;
                    }
                };

                let message = Self::format_message(&enriched, &bot_username);

                match Self::send_message(&bot_token, &chat_id, &message).await {
                    Ok(Some(message_id)) => {
                        // Persiste a mensagem para que o `RugDetectorService`
                        // possa editá-la mais tarde quando o contrato ruggar.
                        let enriched_json = serde_json::to_string(&enriched)
                            .unwrap_or_else(|e| {
                                eprintln!("[TELEGRAM] Falha serializando EnrichedDeploy: {}", e);
                                String::new()
                            });
                        if !enriched_json.is_empty() {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0);
                            if let Err(e) = repo.upsert_sent_message(
                                &enriched.contract_address,
                                &chat_id,
                                message_id,
                                enriched.pair_address.as_deref(),
                                &enriched_json,
                                false,
                                now,
                            ) {
                                eprintln!(
                                    "[TELEGRAM] Falha ao persistir sent_message {}: {}",
                                    enriched.contract_address, e
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        // Caiu no fallback de texto puro — sem message_id parseável.
                        eprintln!(
                            "[TELEGRAM] Mensagem enviada em texto puro para {} (sem rastreio de rug)",
                            enriched.contract_address
                        );
                    }
                    Err(e) => {
                        eprintln!("[TELEGRAM] Erro ao enviar mensagem: {}", e);
                    }
                }
            });
        }

        eprintln!("[TELEGRAM] Worker encerrado — todos os senders foram dropados");
    }

    /// Envia uma mensagem. Retorna `Ok(Some(message_id))` no caminho feliz
    /// (MarkdownV2 aceito) e `Ok(None)` quando precisamos cair pra texto
    /// puro — neste caso ignoramos o tracking porque a mensagem já saiu
    /// quebrada e re-editar não traz o formato de volta.
    pub async fn send_message(
        bot_token: &str,
        chat_id: &str,
        text: &str,
    ) -> Result<Option<i64>, String> {
        let capped = Self::cap_message(text);
        // Tenta MarkdownV2 primeiro. Se a API recusar (geralmente por entity parsing),
        // captura o body com o motivo, loga e refaz como texto puro pra não engolir o token.
        match Self::do_send(bot_token, chat_id, &capped, Some("MarkdownV2")).await {
            Ok(message_id) => Ok(message_id),
            Err(e) => {
                eprintln!("[TELEGRAM] MarkdownV2 falhou: {} — caindo para texto puro", e);
                eprintln!(
                    "[TELEGRAM] >>> conteúdo que falhou:\n---BEGIN---\n{}\n---END---",
                    capped
                );
                Self::do_send(bot_token, chat_id, &capped, None).await?;
                Ok(None)
            }
        }
    }

    /// Edita uma mensagem já enviada. Usado pelo `RugDetectorService`
    /// quando detecta que o contrato foi rugado/honeypot.
    pub async fn edit_message(
        bot_token: &str,
        chat_id: &str,
        message_id: i64,
        text: &str,
    ) -> Result<(), String> {
        let capped = Self::cap_message(text);
        if let Err(e) = Self::do_edit(bot_token, chat_id, message_id, &capped, Some("MarkdownV2")).await {
            eprintln!(
                "[TELEGRAM] editMessageText MarkdownV2 falhou ({}), reenviando em texto puro",
                e
            );
            Self::do_edit(bot_token, chat_id, message_id, &capped, None).await?;
        }
        Ok(())
    }

    async fn do_send(
        bot_token: &str,
        chat_id: &str,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<Option<i64>, String> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
        let client = HttpClient::new();

        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "disable_web_page_preview": true,
        });
        if let Some(mode) = parse_mode {
            body["parse_mode"] = serde_json::Value::String(mode.to_string());
        }

        let resp = client
            .get_client()
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Erro HTTP: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let body_txt = resp.text().await.unwrap_or_default();
            return Err(format!("Telegram API {} body={}", status, body_txt));
        }

        // Parseia { "ok": true, "result": { "message_id": <i64>, ... } }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Erro parseando resposta: {}", e))?;

        let message_id = body
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|m| m.as_i64());

        Ok(message_id)
    }

    async fn do_edit(
        bot_token: &str,
        chat_id: &str,
        message_id: i64,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<(), String> {
        let url = format!("https://api.telegram.org/bot{}/editMessageText", bot_token);
        let client = HttpClient::new();

        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": text,
            "disable_web_page_preview": true,
        });
        if let Some(mode) = parse_mode {
            body["parse_mode"] = serde_json::Value::String(mode.to_string());
        }

        let resp = client
            .get_client()
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Erro HTTP: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let body_txt = resp.text().await.unwrap_or_default();
            // "message is not modified" não é falha de fato.
            if body_txt.contains("message is not modified") {
                return Ok(());
            }
            return Err(format!("Telegram API {} body={}", status, body_txt));
        }

        Ok(())
    }

    fn escape_markdown_v2(text: &str) -> String {
        // Lista oficial do Telegram MarkdownV2 (inclui *, ` e _ que faltavam antes).
        // Esta fun\u00e7\u00e3o s\u00f3 \u00e9 usada para conte\u00fado din\u00e2mico (nome/symbol/labels),
        // n\u00e3o para os delimitadores estruturais que montamos manualmente.
        let escape_chars = "_*[]()~`>#+-=|{}.!";
        let mut result = String::with_capacity(text.len());
        for c in text.chars() {
            if escape_chars.contains(c) {
                result.push('\\');
            }
            result.push(c);
        }
        result
    }

    fn shorten_address(address: &str) -> String {
        if address.len() < 8 {
            return address.to_string();
        }
        format!("0x{}...{}", &address[2..6], &address[address.len()-3..])
    }

    /// Percent-encode dynamic content que entra dentro do `(URL)` de um link
    /// MarkdownV2. O Telegram interrompe o link no primeiro `)` n\u00e3o-escapado,
    /// e n\u00e3o aceita newline na URL — ent\u00e3o precisamos sanitizar.
    fn url_encode(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for b in text.as_bytes() {
            match *b {
                // unreserved (RFC 3986)
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(*b as char);
                }
                _ => out.push_str(&format!("%{:02X}", b)),
            }
        }
        out
    }

    /// Limite "duro" de bytes — Telegram conta 4096 *caracteres após parsing*,
    /// e como nossas mensagens são pesadas em escapes `\X` e `[texto](URL)`
    /// (que após parsing somem), 7-9 KB de bytes ainda cabem confortavelmente.
    /// Só truncamos se ultrapassar este teto bem maior, e o corte é feito num
    /// `\n` balanceado pra não deixar markdown quebrado.
    fn cap_message(text: &str) -> String {
        const MAX: usize = 12_000;
        if text.len() <= MAX {
            return text.to_string();
        }
        let bytes = text.as_bytes();
        let mut depth_paren: i32 = 0;
        let mut depth_brack: i32 = 0;
        let mut last_safe: usize = 0;
        let mut i = 0;
        while i < MAX && i < bytes.len() {
            let b = bytes[i];
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            match b {
                b'[' => depth_brack += 1,
                b']' => depth_brack = (depth_brack - 1).max(0),
                b'(' => depth_paren += 1,
                b')' => depth_paren = (depth_paren - 1).max(0),
                b'\n' if depth_paren == 0 && depth_brack == 0 => {
                    last_safe = i;
                }
                _ => {}
            }
            i += 1;
        }
        if last_safe == 0 {
            return text.to_string();
        }
        format!("{}\n…", &text[..last_safe])
    }

    /// Wrappa um bloco com `~...~` quando `is_scam`. Espelha exatamente o
    /// comportamento do Legacy (cada `XBlock.ts` faz `is_scam ? ~msg~ : msg`).
    /// O wrap é por bloco para evitar quebrar links/`code` que cruzam fronteiras.
    fn maybe_strike(s: String, is_scam: bool) -> String {
        if !is_scam || s.is_empty() {
            return s;
        }
        // Telegram MarkdownV2 strikethrough: `~text~`. Funciona cross-line, e
        // links/code internos seguem renderizando normalmente. Apenas garantimos
        // que conteúdo dinâmico tenha `~` escapado (escape_markdown_v2 já faz).
        format!("~{}~", s)
    }

    pub fn format_message(deploy: &EnrichedDeploy, bot_username: &str) -> String {
        let name = deploy.name.as_deref().unwrap_or("Unknown");
        let symbol = deploy.symbol.as_deref().unwrap_or("???");
        let decimals = deploy.decimals.map(|d| d.to_string()).unwrap_or("?".into());
        let total_supply = deploy.total_supply.as_deref().unwrap_or("?");
        let buy_fee = deploy.buy_fee.as_deref().unwrap_or("?");
        let sell_fee = deploy.sell_fee.as_deref().unwrap_or("?");
        let ca = &deploy.contract_address;
        let is_scam = deploy.is_scam;

        let mut msg = String::new();

        // ── FirstBlock ────────────────────────────────────────────────────
        // Quando `is_scam`, espelha Legacy: prefixa o título com `❌ #RUGGED`
        // (fora do strikethrough, igual ao screenshot do bot legado), e o
        // restante do bloco entra dentro de `~...~`.
        let title_raw = if is_scam {
            format!("❌ #RUGGED {} ({}) - ETH", name, symbol)
        } else {
            format!("{} ({}) - ETH", name, symbol)
        };
        msg.push_str(&Self::escape_markdown_v2(&title_raw));
        msg.push('\n');

        let mut first_block = String::new();
        first_block.push_str(&format!("  • CA:`{}`", ca));
        first_block.push('\n');
        first_block.push_str(&format!("  • Total Supply: {}", Self::escape_markdown_v2(total_supply)));
        first_block.push('\n');
        first_block.push_str(&format!("  • Decimals: {}", Self::escape_markdown_v2(&decimals)));
        first_block.push('\n');

        if let Some(ref mx) = deploy.max_tx {
            // `format_limit` em enrichment_service.rs já devolve o pct
            // com o sufixo "%" (ex.: "100.00%"), então NÃO podemos
            // adicionar outro `%` aqui — caso contrário Telegram renderiza
            // "(100.00%%)". Mantemos só os parênteses escapados.
            let pct = deploy.max_tx_pct.as_deref().unwrap_or("?");
            first_block.push_str(&format!("  • Max Tx: {} \\({}\\)", Self::escape_markdown_v2(mx), Self::escape_markdown_v2(pct)));
            first_block.push('\n');
        }

        if buy_fee != "?" || sell_fee != "?" {
            first_block.push_str(&format!("  • Buy Fee: {}% \\| • Sell Fee: {}%", Self::escape_markdown_v2(buy_fee), Self::escape_markdown_v2(sell_fee)));
            first_block.push('\n');
        }

        if deploy.buy_gas.is_some() || deploy.sell_gas.is_some() {
            let bg_str = deploy.buy_gas.map(|g| format!("\\#G\\_{}", g)).unwrap_or_else(|| "\\-".to_string());
            let sg_str = deploy.sell_gas.map(|g| format!("\\#G\\_{}", g)).unwrap_or_else(|| "\\-".to_string());
            first_block.push_str(&format!("  • Buy Gas: {}  \\|  Sell Gas: {}", bg_str, sg_str));
            first_block.push('\n');
        }
        msg.push_str(&Self::maybe_strike(first_block, is_scam));

        // ── SecondBlock ───────────────────────────────────────────────────
        msg.push('\n');
        let mut second_block = String::new();
        second_block.push_str(&format!("  • Wallet:`{}`", deploy.deployer));
        second_block.push_str(&format!("[🔗](http://etherscan.io/address/{})", deploy.deployer));
        second_block.push('\n');

        let balance_val: f64 = deploy.deployer_balance.parse().unwrap_or(0.0);
        second_block.push_str(&format!("  • Balance:{} ETH", Self::escape_markdown_v2(&format!("{:.2}", balance_val))));
        second_block.push('\n');
        second_block.push_str(&format!("  • Nonce: {}", Self::escape_markdown_v2(&deploy.deployer_nonce)));
        second_block.push('\n');

        if let (Some(ref amount), Some(ref source)) = (&deploy.funding_amount, &deploy.funding_source) {
            let full_address = deploy.funding_source_full.as_deref().unwrap_or(source);
            let amount_val: f64 = amount.parse().unwrap_or(0.0);
            let short = Self::escape_markdown_v2(&Self::shorten_address(full_address));
            let link = format!("[{}](http://etherscan.io/address/{})", short, full_address);
            let bc_ck = deploy.bytecode_checksums.first().map(|e| e.hex.as_str()).unwrap_or("");
            let pencil = format!(
                "[✏](https://t.me/{}?text=%2Fadd%20{}%20{}%20funding%20)",
                bot_username,
                Self::url_encode(full_address),
                Self::url_encode(bc_ck)
            );
            second_block.push_str(&format!("  • Funding: {}` from` {} {} ", Self::escape_markdown_v2(&format!("{:.2}", amount_val)), link, pencil));
            second_block.push('\n');
        }
        msg.push_str(&Self::maybe_strike(second_block, is_scam));

        // ── ThirdBlock - Checksums ────────────────────────────────────────
        msg.push('\n');
        let all_checksums: Vec<&crate::services::enrichment_service::ChecksumEntry> = deploy.bytecode_checksums.iter().chain(deploy.function_checksums.iter()).collect();
        let all_hashes: Vec<String> = all_checksums.iter().map(|e| e.hex.clone()).collect();
        let batch_hashes = all_hashes.join("%20");
        let batch_button = format!(
            "[✏](https://t.me/{}?text=%2FaddBatch%20{}%20{}%20)",
            bot_username,
            Self::url_encode(ca),
            batch_hashes
        );
        // Cabeçalho "🕵️ Checksums" fica fora do strike (igual ao Legacy ThirdBlock,
        // que só riscava o conteúdo dos itens, não o ícone do bloco).
        msg.push_str(&format!("🕵️ Checksums {}\n", batch_button));
        let mut third_block = String::new();
        for entry in &all_checksums {
            let prefix = if entry.is_sub { "    └" } else { "  •" };
            let label = if entry.is_sub {
                format!("{}: ", Self::escape_markdown_v2(&entry.label))
            } else {
                format!("{} ", Self::escape_markdown_v2(&entry.label))
            };
            let scam_pct = if entry.total_count > 0 { (entry.scam_count * 100) / entry.total_count } else { 0 };
            let analytics = format!("*\\({} / {} / {}%\\)*", entry.scam_count, entry.total_count, scam_pct);
            let tag_str = entry.tag.as_ref().map(|t| format!(" {}", Self::escape_markdown_v2(t))).unwrap_or_default();
            let hex_clean = entry.hex.replace("0x", "");
            let tag_button = format!(
                "[✏](https://t.me/{}?text=%2Fadd%20{}%20{}%20)",
                bot_username,
                Self::url_encode(ca),
                Self::url_encode(&entry.hex)
            );

            third_block.push_str(&format!("{} {}\\#0x{} {} {}{}\n", prefix, label, Self::escape_markdown_v2(&hex_clean), analytics, tag_str, tag_button));
        }
        msg.push_str(&Self::maybe_strike(third_block, is_scam));

        // ── FourBlock - Functions ─────────────────────────────────────────
        msg.push('\n');
        msg.push_str("🕵️ Functions\n");
        let mut four_block = String::new();
        let max_funcs = deploy.contract_functions.len().min(23);
        let mut known: Vec<&crate::services::enrichment_service::ContractFunction> = Vec::new();
        let mut unknown: Vec<&crate::services::enrichment_service::ContractFunction> = Vec::new();
        for f in &deploy.contract_functions[..max_funcs] {
            if f.name.is_some() { known.push(f); } else { unknown.push(f); }
        }
        let sorted_funcs: Vec<&&crate::services::enrichment_service::ContractFunction> = known.iter().chain(unknown.iter()).collect();

        for func in sorted_funcs {
            let is_dangerous = func.name.as_ref().map(|n| Self::is_dangerous_function(n)).unwrap_or(false);

            let edit_button = if is_dangerous {
                format!(
                    "[✏](https://t.me/{}?text=%2Fadd%20{}%20{}%20)🐍🐍",
                    bot_username,
                    Self::url_encode(ca),
                    Self::url_encode(&func.selector)
                )
            } else {
                format!(
                    "[✏](https://t.me/{}?text=%2Fadd%20{}%20{}%20)",
                    bot_username,
                    Self::url_encode(ca),
                    Self::url_encode(&func.selector)
                )
            };

            let sig_display = match &func.name {
                Some(name) => format!("  {}", Self::escape_markdown_v2(name)),
                None => "  Unknown".to_string(),
            };

            let tag_display = func.tag.as_ref()
                .map(|t| Self::escape_markdown_v2(t))
                .unwrap_or_default();

            four_block.push_str(&format!(" • `{}`{}{}{}", func.selector, sig_display, edit_button, tag_display));

            if let Some(ref val) = func.return_value {
                if val.len() < 10 {
                    four_block.push_str(&format!("  `{}`\n", val));
                } else {
                    let pct_str = func
                        .return_pct
                        .as_ref()
                        .map(|p| format!(" ({})", p))
                        .unwrap_or_default();
                    four_block.push_str(&format!("\n └ `{}{}`\n", val, pct_str));
                }
            } else {
                four_block.push('\n');
            }
        }
        msg.push_str(&Self::maybe_strike(four_block, is_scam));

        // ── FiveBlock - Verified/Renounced ────────────────────────────────
        msg.push('\n');
        let mut five_block = String::new();
        match deploy.is_verified {
            Some(true) => {
                let compiler = deploy.verified_compiler.as_deref().unwrap_or("unknown");
                five_block.push_str(&format!("🟢 Verified {}\n", Self::escape_markdown_v2(compiler)));

                // Espelha o `verified()` do Legacy (`FiveBlock.ts`):
                // sob a linha "Verified vX.Y.Z" listamos cada rede social
                // encontrada no SourceCode como ` └ Label: <url>`.
                // Ordem casa com o JS (telegram, twitter, web, instagram,
                // tiktok, youtube). Renderizamos como link MarkdownV2 pra
                // manter o URL clicável e blindar caracteres especiais.
                let socials = &deploy.socials;
                let entries: [(&str, &Option<String>); 6] = [
                    ("Telegram", &socials.telegram),
                    ("Twitter", &socials.twitter),
                    ("Web", &socials.web),
                    ("Instagram", &socials.instagram),
                    ("Tiktok", &socials.tiktok),
                    ("Youtube", &socials.youtube),
                ];
                for (label, maybe_url) in entries.iter() {
                    if let Some(url) = maybe_url {
                        let display = Self::escape_markdown_v2(url);
                        // Em MarkdownV2 só `\` e `)` precisam de escape
                        // dentro do parêntese de um link.
                        let target = url.replace('\\', "\\\\").replace(')', "\\)");
                        five_block.push_str(&format!(
                            " └ {}: [{}]({})\n",
                            label, display, target
                        ));
                    }
                }
            }
            Some(false) => five_block.push_str("🔴 Verified\n"),
            None => five_block.push_str("🔴 Verified\n"),
        }

        if deploy.is_renounced {
            five_block.push_str("🟢 Renounced\n");
        } else {
            five_block.push_str("🔴 Renounced\n");
        }
        msg.push_str(&Self::maybe_strike(five_block, is_scam));

        // ── NotBlocks - Annotations ───────────────────────────────────────
        msg.push('\n');
        let bc_ck = deploy.bytecode_checksums.first().map(|e| e.hex.as_str()).unwrap_or("");
        let fn_ck = deploy.function_checksums.first().map(|e| e.hex.as_str()).unwrap_or("");
        let sym_for_template = deploy.symbol.as_deref().unwrap_or("???");

        let annotation_raw = deploy.annotation.as_deref().unwrap_or("").to_string();
        let annotation_escaped = Self::escape_markdown_v2(&annotation_raw).replace("%", "porcento");

        let annotation_display = if annotation_raw.is_empty() {
            String::new()
        } else {
            format!("{}\n", annotation_escaped)
        };

        // If no prior annotation (and no "Verificou" marker), pre-fill the template — matches Legacy
        let pre_msg = if annotation_raw.is_empty() && !annotation_raw.contains("Verificou") {
            format!(
                "{}\n================================================\nTx do dev:\nBundle:\nVerificou contrato antes de lancar:\n================================================",
                sym_for_template
            )
        } else {
            String::new()
        };

        let write_button = format!(
            "[✍🏻](https://t.me/{}?text=%2Fanote%20{}%20{}%20{}%20{}%0A{})",
            bot_username,
            Self::url_encode(ca),
            Self::url_encode(bc_ck),
            Self::url_encode(fn_ck),
            Self::url_encode(&pre_msg),
            Self::url_encode(&annotation_raw)
        );
        let mut not_block = String::new();
        not_block.push_str(&format!("{}Annotation:  {}\n", annotation_display, write_button));

        let gas_annotation_raw = deploy.gas_annotation.as_deref().unwrap_or("").to_string();
        let gas_annotation_display = if gas_annotation_raw.is_empty() {
            String::new()
        } else {
            format!("\n{}\n", Self::escape_markdown_v2(&gas_annotation_raw))
        };
        let gas_write_button = format!(
            "[❌](https://t.me/{}?text=%2FanoteGas%20{}%20{}%20{}%20)",
            bot_username,
            Self::url_encode(ca),
            Self::url_encode(bc_ck),
            Self::url_encode(fn_ck)
        );
        not_block.push_str(&format!("{}Gas Annotations: {}", gas_annotation_display, gas_write_button));

        let sym_str = deploy.symbol.as_deref().unwrap_or("???");
        if let (Some(bg), Some(sg)) = (deploy.buy_gas, deploy.sell_gas) {
            let append_text = format!(
                "/anoteGasAppend {} {} {} {}\n#G_{} | #G_{} ({}) ",
                ca, bc_ck, fn_ck, ca, bg, sg, sym_str
            );
            let append_button = format!(
                "[⛽](https://t.me/{}?text={})",
                bot_username,
                Self::url_encode(&append_text)
            );
            not_block.push_str(&format!("   \\|    {}", append_button));
        }

        if !gas_annotation_raw.is_empty() {
            let edit_text = format!(
                "/anoteGas {} {} {} {}",
                ca, bc_ck, fn_ck, gas_annotation_raw
            );
            let edit_button = format!(
                "[✏](https://t.me/{}?text={})",
                bot_username,
                Self::url_encode(&edit_text)
            );
            not_block.push_str(&format!("   \\|    {}", edit_button));
        }
        msg.push_str(&Self::maybe_strike(not_block, is_scam));
        msg.push_str("\n\n");

        // Links (não vão riscados — combinam com o Legacy, que renderiza os
        // botões de DXS/TWS/etc fora dos blocos com `~`).
        let symbol_url = Self::url_encode(deploy.symbol.as_deref().unwrap_or("???"));
        let ca_url = Self::url_encode(ca);
        msg.push_str(&format!("[DXS](http://dexscreener.com/ethereum/{})  • ", ca_url));
        msg.push_str(&format!("[TWS](https://x.com/search?q=%24{})  • ", symbol_url));
        msg.push_str(&format!("[TWC](https://x.com/search?q={})  • ", ca_url));
        msg.push_str(&format!("[MAE](https://t.me/BananaGunSniper_bot?start=snp_RickSanchez_{})  ", ca_url));
        msg.push_str(&format!(
            "[•](https://t.me/{}?text=%2Fsetsigma) [SIG](https://t.me/SigmaTrading8_bot?text={})  ",
            bot_username, ca_url
        ));
        msg.push_str(&format!(
            "[•](https://t.me/{}?text=%2Fsetbanana) [BAN](https://t.me/BananaGunSniper_bot?text={})",
            bot_username, ca_url
        ));

        msg
    }

    fn is_dangerous_function(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.contains("addbots") ||
        lower.contains("blacklist") ||
        lower.contains("reducefee") ||
        lower.contains("setfee") ||
        lower.contains("blockbot") ||
        lower.contains("antibots") ||
        lower.contains("setblack") ||
        lower.contains("botblock") ||
        lower.contains("setbot") ||
        lower.contains("addbot") ||
        lower.contains("delbots") ||
        lower.contains("decreaseallowance") ||
        lower.contains("increaseallowance")
    }
}
