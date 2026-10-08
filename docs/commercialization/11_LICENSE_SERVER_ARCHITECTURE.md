# 11 License Server Architecture

## Responsibilities
1. entitlement state
2. activation
3. deactivation
4. signed license issuance
5. revocation
6. audit
7. customer support operations

## Components

```text
API
 ├─ /v1/activate
 ├─ /v1/deactivate
 ├─ /v1/entitlements
 └─ /v1/licenses/refresh

Webhook worker
Database
License signer
Admin console
```

## Data model

### customers
id, email, provider_customer_id, created_at

### purchases
id, customer_id, provider_payment_id, product_id, status, amount, currency

### entitlements
id, customer_id, product, plan, status, seat_limit, machine_limit, expires_at

### activations
id, entitlement_id, installation_id, platform, app_version, activated_at, last_seen_at, deactivated_at

### licenses
id, entitlement_id, key_id, payload_hash, issued_at, revoked_at

### audit_events
id, actor, action, entity_type, entity_id, metadata, created_at

## API rules
- rate limit activation
- idempotency keys
- authenticated admin actions
- no raw license private key in DB
- signing service isolated from public API

## Key management
Use KMS/HSM where practical.
Private signing key is never embedded in CI logs or application source.

## Offline token
Signed token includes expiry and feature entitlements.
Client verifies signature locally.

## Observability
Track:
- activation success/failure
- invalid signature attempts
- duplicate activation
- webhook lag
- entitlement mismatch

Never collect raw user files or storage paths.

## Availability
License service outage must not prevent existing paid users from using already-activated offline features within the configured grace period.
