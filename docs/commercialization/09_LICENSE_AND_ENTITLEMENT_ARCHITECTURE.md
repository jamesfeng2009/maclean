# 09 License & Entitlement Architecture

## Objective
Move from a signing-only license implementation toward a commercial entitlement system while preserving offline verification.

## Current-state compatibility
The repository already uses Ed25519-signed licenses with fields such as plan, issue time, machine binding and expiry. Preserve this as the cryptographic trust root.

## Proposed layers

```text
Purchase
  ↓
Payment provider
  ↓
Entitlement service
  ↓
Signed license
  ↓
maclean local verifier
  ↓
Feature gates
```

## License payload
Recommended:
- schema_version
- license_id
- product
- plan
- issued_at
- expires_at
- seats
- machine_limit
- customer_ref
- features[]
- key_id
- signature

Never put payment card data or secrets into the client.

## Activation
1. User enters license key.
2. Client sends key + app version + installation nonce.
3. Server validates.
4. Server issues activation token.
5. Client stores token securely.
6. Local signature verifies every launch.

## Offline behavior
- active token has grace period
- no network required for routine scanning/cleanup
- deactivation requires network when possible
- never brick the app merely because a server is unavailable

## Refund/revocation
Server maintains entitlement status.
Revoked licenses stop new activation.
Existing clients receive a signed revocation/expiry update when online.

## Machine identity
Use a privacy-preserving local installation identifier, not a raw hardware fingerprint where avoidable.
Provide “Deactivate this Mac”.

## Feature flags
License entitlements must not be the only security boundary. Safety remains enabled in all builds.

## Migration
Support legacy Ed25519 payloads until a defined cutoff. Convert on next activation.
