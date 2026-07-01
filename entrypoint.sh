#!/bin/bash

/usr/local/bin/server &
SERVER_PID=$!

sleep 5

until curl -s http://localhost:8080/health > /dev/null 2>&1; do
    sleep 1
done

# Determine WebSocket endpoint
WS_ENDPOINT="${RPC_ENDPOINT:-wss://ethereum-rpc.publicnode.com}"
echo "[STARTUP] Conectando ao RPC Ethereum..."
curl -s -X POST http://localhost:8080/ethers/1/apply_rpc \
  -o /dev/null \
  -H "Content-Type: application/json" \
  --data-binary @- <<EOF
{"endpoint":"${WS_ENDPOINT}","identifier":"eth-mainnet","listen_deploy_event":true}
EOF
echo ""

sleep 2

# Also set RPC endpoint for enrichment/anvil
if [ -n "${RPC_HTTP_ENDPOINT}" ]; then
  echo "[STARTUP] Configurando RPC HTTP para enrichment..."
  curl -s -X POST http://localhost:8080/ethers/1/apply_rpc \
    -o /dev/null \
    -H "Content-Type: application/json" \
    --data-binary @- <<EOF
{"endpoint":"${RPC_HTTP_ENDPOINT}","identifier":"eth-enrichment"}
EOF
  echo ""
fi

echo "[STARTUP] Configurando Telegram..."
curl -s -X POST http://localhost:8080/telegram/configure \
  -o /dev/null \
  -H "Content-Type: application/json" \
  --data-binary @- <<EOF
{"bot_token":"${TELEGRAM_BOT_TOKEN}","chat_id":"${TELEGRAM_CHAT_ID}","bot_username":"${TELEGRAM_BOT_USERNAME:-deployerethmasterbot}","etherscan_api_key":"${ETHERSCAN_API_KEY}"}
EOF
echo ""

sleep 2

echo "[STARTUP] Ativando listener de deploys ERC-20..."
curl -s -X POST http://localhost:8080/ethers/1/listen_deploy_erc20 \
  -o /dev/null \
  -H "Content-Type: application/json" \
  -d '{"webhook": "http://localhost:8080/health"}'
echo ""

echo "[STARTUP] Configuracao completa!"

wait $SERVER_PID
