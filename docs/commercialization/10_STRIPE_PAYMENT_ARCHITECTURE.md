# 10 Stripe Payment Architecture

## Goal
Use Stripe as the payment authority while keeping the maclean client independent from Stripe APIs.

## Principle
Stripe proves payment. The License Service proves product entitlement.

```text
Website
  ↓
Checkout
  ↓
Stripe
  ↓ webhook
Billing Adapter
  ↓
Entitlement Service
  ↓
License Issuer
```

## Products
- MACLEAN_PRO_FOUNDING
- MACLEAN_PRO_PERSONAL
- MACLEAN_TEAM_MONTHLY
- optional upgrade SKU

Do not encode prices directly into the client.

## Checkout
Website creates a server-side checkout session.
Never trust client-supplied price/product IDs.

## Webhooks
Required logical events:
- checkout completed
- payment succeeded
- payment failed
- refund
- dispute
- subscription created/updated/canceled for Team

All webhook handlers must be:
- signature verified
- idempotent
- persisted
- replay-safe

## Entitlement state
`pending → active → grace → revoked`

For lifetime:
`pending → active → refunded/revoked`

## Refund behavior
Refund triggers entitlement revocation according to policy.
Do not immediately brick an installed client if an offline grace policy applies.

## Privacy
Store only:
- Stripe customer ID
- payment object IDs
- product/price ID
- entitlement status
- email if needed for license delivery
- timestamps

Do not store card details.

## Tax
Use Stripe Tax or another compliant tax system after validating jurisdictional requirements.

## Security
Webhook secret is server-only.
Stripe secret keys never ship in maclean.
