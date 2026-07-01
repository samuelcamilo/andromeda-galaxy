use rusqlite::{params, params_from_iter, Connection, Result as SqliteResult};
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SqliteRepositoryError {
    #[error("Erro SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Erro de serialização: {0}")]
    Serde(#[from] serde_json::Error),
}

pub struct SqliteRepository {
    conn: Mutex<Connection>,
}

impl SqliteRepository {
    pub fn new(path: &str) -> Result<Self, SqliteRepositoryError> {
        let conn = Connection::open(path)?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS labels (
                address TEXT NOT NULL,
                chain_id INTEGER NOT NULL,
                label TEXT NOT NULL,
                name_tag TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_labels_address ON labels(address);

            CREATE TABLE IF NOT EXISTS checksum_contracts (
                address TEXT PRIMARY KEY,
                network_id INTEGER NOT NULL,
                extra_fields TEXT NOT NULL DEFAULT '{}'
            );

            CREATE TABLE IF NOT EXISTS checksum_history (
                checksum_hex TEXT PRIMARY KEY,
                scam_count INTEGER NOT NULL DEFAULT 0,
                total_count INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS signatures (
                hex_signature TEXT PRIMARY KEY,
                text_signature TEXT NOT NULL,
                timestamp REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS erc20_deployments (
                hash TEXT PRIMARY KEY,
                data TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS indicators (
                checksum TEXT PRIMARY KEY,
                tag TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS annotations (
                checksum TEXT PRIMARY KEY,
                text TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS gas_annotations (
                checksum TEXT PRIMARY KEY,
                text TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ignores (
                checksum TEXT PRIMARY KEY
            );

            CREATE TABLE IF NOT EXISTS bot_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Mensagens já postadas no Telegram que ainda podem ser editadas
            -- pelo `RugDetectorService` quando o contrato é rugado/honeypot.
            -- Guardamos o `EnrichedDeploy` serializado para conseguir
            -- reformatar a mensagem com `is_scam = true` sem precisar
            -- re-enriquecer (que é caro: Anvil, Etherscan, RPC etc).
            CREATE TABLE IF NOT EXISTS sent_messages (
                contract_address TEXT PRIMARY KEY,
                chat_id TEXT NOT NULL,
                message_id INTEGER NOT NULL,
                pair_address TEXT,
                enriched_json TEXT NOT NULL,
                is_scam INTEGER NOT NULL DEFAULT 0,
                sent_at INTEGER NOT NULL,
                last_checked_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_sent_messages_scam_check
                ON sent_messages(is_scam, last_checked_at);",
        )?;

        // Migrações idempotentes pra adicionar colunas em DBs já existentes.
        // SQLite não tem `ADD COLUMN IF NOT EXISTS`, então tentamos e
        // engolimos o erro `duplicate column name` quando a coluna já está lá.
        let migrations: &[&str] = &[
            // ATH (FDV) histórico do token, capturado via `GeckoTerminalService`
            // (api.geckoterminal.com — fonte alternativa ao DexScreener cujo
            // endpoint de chart é bloqueado pelo Cloudflare em IPs de
            // datacenter). Cache permanente: o ATH só sobe, então um único
            // fetch por contrato basta. Pode ser invalidado/forçado novo
            // fetch zerando `last_dex_check_at`.
            "ALTER TABLE sent_messages ADD COLUMN ath_market_cap_usd REAL NOT NULL DEFAULT 0",
            "ALTER TABLE sent_messages ADD COLUMN ath_price_usd REAL NOT NULL DEFAULT 0",
            "ALTER TABLE sent_messages ADD COLUMN ath_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE sent_messages ADD COLUMN last_dex_check_at INTEGER NOT NULL DEFAULT 0",
        ];
        for stmt in migrations {
            if let Err(e) = conn.execute(stmt, []) {
                let msg = e.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(SqliteRepositoryError::Sqlite(e));
                }
            }
        }
        // Index pro AthTrackerService achar contratos pra revisitar (LRU).
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_sent_messages_dex_check
                ON sent_messages(last_dex_check_at);",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // --- Labels ---

    pub fn search_labels_by_address(
        &self,
        address: &str,
    ) -> Result<Vec<LabelRow>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT address, chain_id, label, name_tag FROM labels WHERE address = ?1",
        )?;
        let rows = stmt
            .query_map(params![address], |row| {
                Ok(LabelRow {
                    address: row.get(0)?,
                    chain_id: row.get(1)?,
                    label: row.get(2)?,
                    name_tag: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // --- Checksums ---

    pub fn upsert_checksum(
        &self,
        address: &str,
        network_id: i32,
        extra_fields: &str,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO checksum_contracts (address, network_id, extra_fields)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(address) DO UPDATE SET network_id=?2, extra_fields=?3",
            params![address, network_id, extra_fields],
        )?;
        Ok(())
    }

    pub fn get_checksum(
        &self,
        address: &str,
    ) -> Result<Option<ChecksumRow>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT address, network_id, extra_fields FROM checksum_contracts WHERE address = ?1",
        )?;
        let mut rows = stmt.query_map(params![address], |row| {
            Ok(ChecksumRow {
                address: row.get(0)?,
                network_id: row.get(1)?,
                extra_fields: row.get(2)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn checksum_count(
        &self,
        field_name: &str,
        field_value: &str,
        check_rug: Option<bool>,
    ) -> Result<u64, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();

        if field_name == "address" || field_name == "network_id" {
            let base_query = format!(
                "SELECT COUNT(*) FROM checksum_contracts WHERE {} = ?1",
                field_name
            );

            if check_rug == Some(true) {
                let query = format!(
                    "SELECT COUNT(*) FROM checksum_contracts WHERE {} = ?1 AND json_extract(extra_fields, '$.is_scam') = 1",
                    field_name
                );
                let count: u64 = conn.query_row(&query, params![field_value], |row| row.get(0))?;
                return Ok(count);
            }

            let count: u64 =
                conn.query_row(&base_query, params![field_value], |row| row.get(0))?;
            return Ok(count);
        }

        let query = format!(
            "SELECT COUNT(*) FROM checksum_contracts WHERE json_extract(extra_fields, '$.{}') = ?1",
            field_name
        );

        let full_query = if check_rug == Some(true) {
            format!(
                "{} AND json_extract(extra_fields, '$.is_scam') = 1",
                query
            )
        } else {
            query
        };

        let count: u64 =
            conn.query_row(&full_query, params![field_value], |row| row.get(0))?;
        Ok(count)
    }

    // --- Checksum History ---

    pub fn get_checksum_history(
        &self,
        checksum_hex: &str,
    ) -> Result<(u64, u64), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT scam_count, total_count FROM checksum_history WHERE checksum_hex = ?1",
            params![checksum_hex],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        );
        match result {
            Ok((scam, total)) => Ok((scam, total)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok((0, 0)),
            Err(e) => Err(e.into()),
        }
    }

    pub fn upsert_checksum_history(
        &self,
        checksum_hex: &str,
        scam_count: u64,
        total_count: u64,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO checksum_history (checksum_hex, scam_count, total_count)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(checksum_hex) DO UPDATE SET scam_count=?2, total_count=?3",
            params![checksum_hex, scam_count, total_count],
        )?;
        Ok(())
    }

    pub fn increment_checksum_history(
        &self,
        checksum_hex: &str,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO checksum_history (checksum_hex, scam_count, total_count)
             VALUES (?1, 0, 1)
             ON CONFLICT(checksum_hex) DO UPDATE SET total_count = total_count + 1",
            params![checksum_hex],
        )?;
        Ok(())
    }

    pub fn clear_checksum_history(&self) -> Result<usize, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute("DELETE FROM checksum_history", [])?;
        Ok(deleted)
    }

    // --- Signatures ---

    pub fn upsert_signature(
        &self,
        hex_signature: &str,
        text_signature: &str,
        timestamp: f64,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO signatures (hex_signature, text_signature, timestamp)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(hex_signature) DO UPDATE SET text_signature=?2, timestamp=?3",
            params![hex_signature, text_signature, timestamp],
        )?;
        Ok(())
    }

    pub fn get_signatures_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<SignatureRow>, SqliteRepositoryError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT hex_signature, text_signature, timestamp FROM signatures WHERE hex_signature IN ({})",
            placeholders
        );

        let mut stmt = conn.prepare(&query)?;
        let rows = stmt
            .query_map(params_from_iter(ids.iter()), |row| {
                Ok(SignatureRow {
                    hex_signature: row.get(0)?,
                    text_signature: row.get(1)?,
                    timestamp: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // --- ERC20 Deployments ---

    pub fn deployment_exists(&self, hash: &str) -> Result<bool, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let count: u64 = conn.query_row(
            "SELECT COUNT(*) FROM erc20_deployments WHERE hash = ?1",
            params![hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn bulk_insert_deployments(
        &self,
        deployments: &[(String, String)],
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = conn.prepare(
                "INSERT OR IGNORE INTO erc20_deployments (hash, data) VALUES (?1, ?2)",
            )?;
            for (hash, data) in deployments {
                stmt.execute(params![hash, data])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // --- Indicators ---

    pub fn set_indicator(&self, checksum: &str, tag: &str) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO indicators (checksum, tag) VALUES (?1, ?2)
             ON CONFLICT(checksum) DO UPDATE SET tag=?2",
            params![checksum, tag],
        )?;
        Ok(())
    }

    pub fn del_indicator(&self, checksum: &str) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM indicators WHERE checksum = ?1",
            params![checksum],
        )?;
        Ok(())
    }

    pub fn clear_indicators(&self) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM indicators", [])?;
        Ok(())
    }

    pub fn get_indicators(&self) -> Result<HashMap<String, String>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT checksum, tag FROM indicators")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (k, v) = row?;
            map.insert(k, v);
        }
        Ok(map)
    }

    // --- Annotations ---

    pub fn set_annotation(
        &self,
        checksum: &str,
        text: &str,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO annotations (checksum, text) VALUES (?1, ?2)
             ON CONFLICT(checksum) DO UPDATE SET text=?2",
            params![checksum, text],
        )?;
        Ok(())
    }

    pub fn append_annotation(
        &self,
        checksum: &str,
        text: &str,
    ) -> Result<String, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let existing: String = conn
            .query_row(
                "SELECT text FROM annotations WHERE checksum = ?1",
                params![checksum],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let new_text = if existing.is_empty() {
            text.to_string()
        } else {
            format!("{}\n{}", existing, text)
        };
        conn.execute(
            "INSERT INTO annotations (checksum, text) VALUES (?1, ?2)
             ON CONFLICT(checksum) DO UPDATE SET text=?2",
            params![checksum, &new_text],
        )?;
        Ok(new_text)
    }

    pub fn del_annotation(&self, checksum: &str) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM annotations WHERE checksum = ?1",
            params![checksum],
        )?;
        Ok(())
    }

    pub fn get_annotations(&self) -> Result<HashMap<String, String>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT checksum, text FROM annotations")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (k, v) = row?;
            map.insert(k, v);
        }
        Ok(map)
    }

    // --- Gas Annotations ---

    pub fn set_gas_annotation(
        &self,
        checksum: &str,
        text: &str,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO gas_annotations (checksum, text) VALUES (?1, ?2)
             ON CONFLICT(checksum) DO UPDATE SET text=?2",
            params![checksum, text],
        )?;
        Ok(())
    }

    pub fn append_gas_annotation(
        &self,
        checksum: &str,
        text: &str,
    ) -> Result<String, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let existing: String = conn
            .query_row(
                "SELECT text FROM gas_annotations WHERE checksum = ?1",
                params![checksum],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let new_text = if existing.is_empty() {
            text.to_string()
        } else {
            format!("{}\n{}", existing, text)
        };
        conn.execute(
            "INSERT INTO gas_annotations (checksum, text) VALUES (?1, ?2)
             ON CONFLICT(checksum) DO UPDATE SET text=?2",
            params![checksum, &new_text],
        )?;
        Ok(new_text)
    }

    pub fn del_gas_annotation(&self, checksum: &str) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM gas_annotations WHERE checksum = ?1",
            params![checksum],
        )?;
        Ok(())
    }

    pub fn get_gas_annotations(&self) -> Result<HashMap<String, String>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT checksum, text FROM gas_annotations")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (k, v) = row?;
            map.insert(k, v);
        }
        Ok(map)
    }

    // --- Ignores ---

    pub fn add_ignore(&self, checksum: &str) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO ignores (checksum) VALUES (?1)",
            params![checksum],
        )?;
        Ok(())
    }

    pub fn rm_ignore(&self, checksum: &str) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM ignores WHERE checksum = ?1",
            params![checksum],
        )?;
        Ok(())
    }

    pub fn get_ignores(&self) -> Result<Vec<String>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT checksum FROM ignores")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // --- Bot Settings ---

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO bot_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=?2",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM bot_settings WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |row| row.get(0))?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    // --- Sent Messages (rug-detector tracking) ---

    pub fn upsert_sent_message(
        &self,
        contract_address: &str,
        chat_id: &str,
        message_id: i64,
        pair_address: Option<&str>,
        enriched_json: &str,
        is_scam: bool,
        sent_at: i64,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sent_messages
                (contract_address, chat_id, message_id, pair_address, enriched_json, is_scam, sent_at, last_checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
             ON CONFLICT(contract_address) DO UPDATE SET
                chat_id=?2,
                message_id=?3,
                pair_address=?4,
                enriched_json=?5,
                is_scam=?6,
                sent_at=?7",
            params![
                contract_address.to_lowercase(),
                chat_id,
                message_id,
                pair_address,
                enriched_json,
                if is_scam { 1 } else { 0 },
                sent_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_pending_scam_checks(
        &self,
        limit: u32,
    ) -> Result<Vec<SentMessageRow>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT contract_address, chat_id, message_id, pair_address, enriched_json, is_scam, sent_at, last_checked_at
             FROM sent_messages
             WHERE is_scam = 0
             ORDER BY last_checked_at ASC, sent_at ASC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(SentMessageRow {
                    contract_address: row.get(0)?,
                    chat_id: row.get(1)?,
                    message_id: row.get(2)?,
                    pair_address: row.get(3)?,
                    enriched_json: row.get(4)?,
                    is_scam: row.get::<_, i64>(5)? != 0,
                    sent_at: row.get(6)?,
                    last_checked_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_sent_message_scam(
        &self,
        contract_address: &str,
        is_scam: bool,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sent_messages SET is_scam = ?2 WHERE contract_address = ?1",
            params![contract_address.to_lowercase(), if is_scam { 1 } else { 0 }],
        )?;
        Ok(())
    }

    pub fn touch_sent_message_check(
        &self,
        contract_address: &str,
        checked_at: i64,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sent_messages SET last_checked_at = ?2 WHERE contract_address = ?1",
            params![contract_address.to_lowercase(), checked_at],
        )?;
        Ok(())
    }

    pub fn delete_sent_message(
        &self,
        contract_address: &str,
    ) -> Result<(), SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM sent_messages WHERE contract_address = ?1",
            params![contract_address.to_lowercase()],
        )?;
        Ok(())
    }

    /// Lê apenas `(contract_address, enriched_json)` de **todos** os
    /// deploys já notificados — usado pelo `/compare <addr>` (modo busca)
    /// pra rankear similaridade contra o histórico.
    ///
    /// Os JSONs costumam ter ~2-5 KB cada; mesmo com milhares de linhas
    /// o custo é baixo porque o parse + Jaccard rodam em memória.
    pub fn list_all_enriched_brief(
        &self,
    ) -> Result<Vec<(String, String)>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT contract_address, enriched_json
             FROM sent_messages
             ORDER BY sent_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Lê o ATH cacheado pra um contrato. Retorna `None` se nunca foi
    /// trackeado, ou se o registro existe mas tem ath_market_cap_usd = 0.
    /// Usado pelo `/compare` antes de chamar GeckoTerminal — economiza ~1s
    /// e respeita o rate limit (30 req/min) quando o usuário roda /compare
    /// repetidamente.
    pub fn get_ath(
        &self,
        contract_address: &str,
    ) -> Result<Option<AthRow>, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let res = conn.query_row(
            "SELECT ath_market_cap_usd, ath_price_usd, ath_at, last_dex_check_at
             FROM sent_messages
             WHERE contract_address = ?1",
            params![contract_address.to_lowercase()],
            |row| {
                Ok(AthRow {
                    ath_market_cap_usd: row.get(0)?,
                    ath_price_usd: row.get(1)?,
                    ath_at: row.get(2)?,
                    last_dex_check_at: row.get(3)?,
                })
            },
        );
        match res {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Persiste o ATH (e timestamp da checagem) pra um contrato.
    /// Idempotente — se o registro não existe em `sent_messages` (caso
    /// de um endereço passado no `/compare` que nunca gerou card), retorna
    /// 0 linhas afetadas e o caller pode ignorar.
    pub fn set_ath(
        &self,
        contract_address: &str,
        ath_market_cap_usd: f64,
        ath_price_usd: f64,
        ath_at: i64,
        last_dex_check_at: i64,
    ) -> Result<usize, SqliteRepositoryError> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE sent_messages
             SET ath_market_cap_usd = ?2,
                 ath_price_usd      = ?3,
                 ath_at             = ?4,
                 last_dex_check_at  = ?5
             WHERE contract_address = ?1",
            params![
                contract_address.to_lowercase(),
                ath_market_cap_usd,
                ath_price_usd,
                ath_at,
                last_dex_check_at,
            ],
        )?;
        Ok(n)
    }
}

#[derive(Debug, Clone)]
pub struct SentMessageRow {
    pub contract_address: String,
    pub chat_id: String,
    pub message_id: i64,
    pub pair_address: Option<String>,
    pub enriched_json: String,
    pub is_scam: bool,
    pub sent_at: i64,
    pub last_checked_at: i64,
}

#[derive(Debug, Clone)]
pub struct LabelRow {
    pub address: String,
    pub chain_id: u32,
    pub label: String,
    pub name_tag: String,
}

#[derive(Debug, Clone)]
pub struct ChecksumRow {
    pub address: String,
    pub network_id: i32,
    pub extra_fields: String,
}

#[derive(Debug, Clone, Default)]
pub struct AthRow {
    pub ath_market_cap_usd: f64,
    pub ath_price_usd: f64,
    pub ath_at: i64,
    pub last_dex_check_at: i64,
}

#[derive(Debug, Clone)]
pub struct SignatureRow {
    pub hex_signature: String,
    pub text_signature: String,
    pub timestamp: f64,
}
