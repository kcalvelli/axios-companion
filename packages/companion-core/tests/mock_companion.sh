#!/usr/bin/env bash
# Mock companion script for integration tests.
# Emits canned stream-json output that mimics real claude behavior.
#
# Modes (set via MOCK_MODE env var):
#   normal   — legacy: init + assistant + result/success (default)
#   error    — init + result/error
#   crash    — exits non-zero with no stream-json
#   slow     — like normal but sleeps 2s before result (for cancellation tests)
#   partial  — init + stream_event/content_block_delta deltas + the legacy
#              assistant event (which the dispatcher must dedupe against the
#              streamed deltas) + result/success. Mimics real claude with
#              --include-partial-messages.
#   argv     — like normal, but FIRST writes the full received argv
#              (one arg per line, NUL-separator unsafe but fine for tests)
#              to $MOCK_ARGV_FILE. Used by the trust-level regression
#              tests to assert the dispatcher passed --permission-mode
#              dontAsk and the right --settings JSON.

MODE="${MOCK_MODE:-normal}"
SESSION_ID="${MOCK_SESSION_ID:-deadbeef-1234-5678-9abc-def012345678}"

case "$MODE" in
  normal)
    echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'","model":"claude-sonnet-4-20250514"}'
    echo '{"type":"assistant","message":{"content":[{"text":"Hello from "}]}}'
    echo '{"type":"assistant","message":{"content":[{"text":"mock companion."}]}}'
    echo '{"type":"result","subtype":"success","result":"Hello from mock companion."}'
    ;;
  error)
    echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'","model":"claude-sonnet-4-20250514"}'
    echo '{"type":"result","subtype":"error","error":"something went wrong"}'
    ;;
  crash)
    exit 1
    ;;
  slow)
    echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'","model":"claude-sonnet-4-20250514"}'
    echo '{"type":"assistant","message":{"content":[{"text":"thinking..."}]}}'
    sleep 2
    echo '{"type":"result","subtype":"success","result":"thinking..."}'
    ;;
  argv)
    if [ -n "$MOCK_ARGV_FILE" ]; then
      : > "$MOCK_ARGV_FILE"
      for a in "$@"; do
        printf '%s\n' "$a" >> "$MOCK_ARGV_FILE"
      done
    fi
    echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'","model":"claude-sonnet-4-20250514"}'
    echo '{"type":"assistant","message":{"content":[{"text":"argv mock"}]}}'
    echo '{"type":"result","subtype":"success","result":"argv mock"}'
    ;;
  partial)
    echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'","model":"claude-sonnet-4-20250514"}'
    echo '{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_test","type":"message","role":"assistant","content":[]}}}'
    echo '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}'
    echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello "}}}'
    echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"streaming "}}}'
    echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"world"}}}'
    echo '{"type":"assistant","message":{"content":[{"text":"Hello streaming world"}]}}'
    echo '{"type":"stream_event","event":{"type":"content_block_stop","index":0}}'
    echo '{"type":"stream_event","event":{"type":"message_stop"}}'
    echo '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}'
    echo '{"type":"result","subtype":"success","result":"Hello streaming world"}'
    ;;
esac
