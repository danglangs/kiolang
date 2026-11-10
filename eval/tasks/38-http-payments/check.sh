#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
serve db:db="$T/pay.db"
P1='{"id":"pay_1","account":"acme","cents":1250,"currency":"EUR","status":"pending","refunded":0}'
check_req POST /payments '{"account": "acme", "cents": 1250, "currency": "EUR"}' -- '400 {"error":"missing idempotency key"}'
HDR="idempotency-key: k1" check_req POST /payments '{"account": "acme", "cents": 1250, "currency": "EUR"}' -- "201 $P1"
HDR="idempotency-key: k1" check_req POST /payments '{"currency":"EUR","cents":1250,"account":"acme"}' -- "201 $P1"
HDR="idempotency-key: k1" check_req POST /payments '{"account": "acme", "cents": 1251, "currency": "EUR"}' -- '409 {"error":"key reused"}'
HDR="idempotency-key: k2" check_req POST /payments '{"account": "acme", "cents": 0, "currency": "EUR"}' -- '422 {"error":"invalid payment"}'
HDR="idempotency-key: k2" check_req POST /payments '{"account": "acme", "cents": 300, "currency": "USD"}' -- '201 {"id":"pay_2","account":"acme","cents":300,"currency":"USD","status":"pending","refunded":0}'
HDR="idempotency-key: k3" check_req POST /payments '{"account": "acme", "cents": 5, "currency": "GBP"}' -- '422 {"error":"invalid payment"}'
HDR="idempotency-key: k3" check_req POST /payments 'not json' -- '422 {"error":"invalid payment"}'
HDR="idempotency-key: k3" check_req POST /payments '{"account": "acme", "cents": 1.5, "currency": "EUR"}' -- '422 {"error":"invalid payment"}'
HDR="idempotency-key: k3" check_req POST /payments '{"account": "acme", "cents": true, "currency": "EUR"}' -- '422 {"error":"invalid payment"}'
HDR="idempotency-key: k3" check_req POST /payments '{"account": "", "cents": 5, "currency": "EUR"}' -- '422 {"error":"invalid payment"}'
HDR="idempotency-key: k3" check_req POST /payments '{"account": "acme", "cents": 5, "currency": "EUR", "note": "x"}' -- '422 {"error":"invalid payment"}'
HDR="idempotency-key: k6" check_req POST /payments '{"account": "ünï", "cents": 1, "currency": "USD"}' -- '201 {"id":"pay_3","account":"ünï","cents":1,"currency":"USD","status":"pending","refunded":0}'
check_req GET /payments/pay_1 -- "200 $P1"
check_req GET /payments/pay_9 -- '404 {"error":"not found"}'
check_req GET /payments/1 -- '404 {"error":"not found"}'
C1='{"id":"pay_1","account":"acme","cents":1250,"currency":"EUR","status":"captured","refunded":0}'
check_req POST /payments/pay_1/capture -- "200 $C1"
check_req POST /payments/pay_1/capture -- "200 $C1"
check_req POST /payments/pay_1/cancel -- '409 {"error":"invalid transition"}'
check_req POST /payments/pay_2/cancel -- '200 {"id":"pay_2","account":"acme","cents":300,"currency":"USD","status":"canceled","refunded":0}'
check_req POST /payments/pay_2/capture -- '409 {"error":"invalid transition"}'
check_req POST /payments/pay_8/capture -- '404 {"error":"not found"}'
R1='{"id":"pay_1","account":"acme","cents":1250,"currency":"EUR","status":"captured","refunded":250}'
check_req POST /payments/pay_1/refund '{"cents": 250}' -- '400 {"error":"missing idempotency key"}'
HDR="idempotency-key: r1" check_req POST /payments/pay_1/refund '{"cents": 250}' -- "200 $R1"
HDR="idempotency-key: r1" check_req POST /payments/pay_1/refund '{"cents":250}' -- "200 $R1"
HDR="idempotency-key: r1" check_req POST /payments/pay_3/refund '{"cents": 250}' -- '409 {"error":"key reused"}'
HDR="idempotency-key: k1" check_req POST /payments/pay_1/refund '{"cents": 1}' -- '409 {"error":"key reused"}'
HDR="idempotency-key: r2" check_req POST /payments/pay_1/refund '{"cents": 1001}' -- '409 {"error":"exceeds captured"}'
HDR="idempotency-key: r2" check_req POST /payments/pay_1/refund '{"cents": -1}' -- '422 {"error":"invalid refund"}'
HDR="idempotency-key: r2" check_req POST /payments/pay_7/refund '{"cents": 1}' -- '404 {"error":"not found"}'
HDR="idempotency-key: r2" check_req POST /payments/pay_1/refund '{"cents": 1000}' -- '200 {"id":"pay_1","account":"acme","cents":1250,"currency":"EUR","status":"refunded","refunded":1250}'
HDR="idempotency-key: r4" check_req POST /payments/pay_1/refund '{"cents": 1}' -- '409 {"error":"invalid transition"}'
HDR="idempotency-key: r5" check_req POST /payments/pay_3/refund '{"cents": 1}' -- '409 {"error":"invalid transition"}'
check_req GET '/payments?account=acme' -- '200 [{"id":"pay_1","account":"acme","cents":1250,"currency":"EUR","status":"refunded","refunded":1250},{"id":"pay_2","account":"acme","cents":300,"currency":"USD","status":"canceled","refunded":0}]'
check_req GET '/payments?account=nobody' -- '200 []'
stop

serve db:db="$T/pay.db"
HDR="idempotency-key: k1" check_req POST /payments '{"account": "acme", "cents": 1250, "currency": "EUR"}' -- "201 $P1"
HDR="idempotency-key: r1" check_req POST /payments/pay_1/refund '{"cents": 250}' -- "200 $R1"
HDR="idempotency-key: k7" check_req POST /payments '{"account": "acme", "cents": 9, "currency": "EUR"}' -- '201 {"id":"pay_4","account":"acme","cents":9,"currency":"EUR","status":"pending","refunded":0}'
check_req DELETE /payments/pay_1 -- '404 {"error":"not found"}'
check_req GET /payments/pay_1/capture -- '404 {"error":"not found"}'
