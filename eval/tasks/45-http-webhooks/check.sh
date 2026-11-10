#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture secrets secrets
start() { serve db:db="$T/hooks.db" dir:secrets="$T/secrets" log:log="$T/hooks.log"; }
sig() { python3 -c 'import hashlib, sys; key = open(sys.argv[1], "rb").read().rstrip(b"\n")
print(hashlib.sha256(key + b"." + sys.argv[2].encode()).hexdigest())' "$T/secrets/webhook.key" "$1"; }
ev() { echo "{\"id\": \"$1\", \"type\": \"$2\", \"subscription\": \"$3\", \"seq\": $4}"; }
# send BODY WANT: POST the body, signed.
send() { HDR="x-signature: sha256=$(sig "$1")" check_req POST /webhooks "$1" -- "$2"; }

start
send "$(ev evt_1 subscription.created sub_1 1)" '200 {"status":"applied","applied":1}'
send "$(ev evt_1 subscription.created sub_1 1)" '200 {"status":"duplicate"}'
send "$(ev evt_3 payment.failed sub_1 3)" '202 {"status":"queued"}'
send "$(ev evt_4 subscription.activated sub_1 4)" '202 {"status":"queued"}'
send "$(ev evt_2 subscription.activated sub_1 2)" '200 {"status":"applied","applied":3}'
check_req GET /subscriptions/sub_1 -- '200 {"id":"sub_1","state":"active","seq":4,"history":["created","active","past_due","active"]}'
body=$(ev evt_x subscription.canceled sub_1 5)
HDR="x-signature: sha256=0000000000000000000000000000000000000000000000000000000000000000" check_req POST /webhooks "$body" -- '401 {"error":"bad signature"}'
check_req POST /webhooks "$body" -- '401 {"error":"bad signature"}'
HDR="x-signature: sha256=$(sig "$(ev evt_x subscription.canceled sub_1 6)")" check_req POST /webhooks "$body" -- '401 {"error":"bad signature"}'
HDR="x-signature: $(sig "$body")" check_req POST /webhooks "$body" -- '401 {"error":"bad signature"}'
send "$(ev evt_y subscription.created sub_1 0)" '400 {"error":"invalid event"}'
send "$(ev evt_y subscription.paused sub_1 5)" '400 {"error":"invalid event"}'
send '{"id": "evt_y", "type": "payment.failed", "seq": 5}' '400 {"error":"invalid event"}'
send '{"id": "evt_y", "type": "payment.failed", "subscription": "sub_1", "seq": "5"}' '400 {"error":"invalid event"}'
send 'not json' '400 {"error":"invalid event"}'
send "$(ev evt_5 payment.failed sub_2 1)" '200 {"status":"applied","applied":1}'
check_req GET /subscriptions/sub_2 -- '200 {"id":"sub_2","state":"none","seq":1,"history":[]}'
send "$(ev evt_6 subscription.canceled sub_1 2)" '200 {"status":"stale"}'
send "$(ev evt_6 subscription.canceled sub_1 2)" '200 {"status":"duplicate"}'
send "$(ev evt_7 subscription.canceled sub_1 6)" '202 {"status":"queued"}'
send "$(ev evt_8 payment.failed sub_1 5)" '200 {"status":"applied","applied":2}'
send "$(ev evt_9 subscription.activated sub_1 7)" '200 {"status":"applied","applied":1}'
check_req GET /subscriptions/sub_1 -- '200 {"id":"sub_1","state":"canceled","seq":7,"history":["created","active","past_due","active","past_due","canceled"]}'
send "$(ev evt_10 subscription.activated sub_3 2)" '202 {"status":"queued"}'
send "$(ev evt_10 subscription.activated sub_3 2)" '200 {"status":"duplicate"}'
check_req GET /subscriptions/sub_3 -- '404 {"error":"not found"}'
check_req GET /subscriptions/sub_9 -- '404 {"error":"not found"}'
check_req GET /webhooks -- '404 {"error":"not found"}'
check_req DELETE /subscriptions/sub_1 -- '404 {"error":"not found"}'
stop

start
check_req GET /subscriptions/sub_1 -- '200 {"id":"sub_1","state":"canceled","seq":7,"history":["created","active","past_due","active","past_due","canceled"]}'
send "$(ev evt_11 subscription.created sub_3 1)" '200 {"status":"applied","applied":2}'
check_req GET /subscriptions/sub_3 -- '200 {"id":"sub_3","state":"active","seq":2,"history":["created","active"]}'
send "$(ev evt_4 subscription.activated sub_1 4)" '200 {"status":"duplicate"}'
stop

sed -E 's/^req [0-9]+: //' "$T/hooks.log" >"$T/log.txt" 2>/dev/null || fail "no log written"
expect_file "$T/log.txt" want.log
