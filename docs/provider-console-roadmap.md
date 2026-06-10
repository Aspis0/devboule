# Provider Console Roadmap

Status: first implementation slice, 2026-05-28.

Goal: rebuild the useful parts of Cloudflare and Scaleway consoles inside Aspis Management with safer defaults, clearer grouping, and Aspis Bio scope isolation.

## Current Live Surface

Cloudflare:
- Pinned account scope for Aspis Bio, with warning when the Cloudflare account display name is not `aspis-bio`.
- Workers inventory allowlisted to Aspis Bio workers only; sibling account workers are hidden from mutation surfaces.
- Worker routes, deployments, compatibility metadata and guarded secret rotation.
- Best-effort lists and counts for R2 buckets, D1 databases, KV namespaces, Queues and Vectorize indexes.
- Best-effort lists for Pages projects, Zones, DNS records, zone rulesets, Access applications and Cloudflare Tunnels.
- Best-effort lists for account audit logs, AI Search namespaces/instances, AI Gateway gateways and Logpush jobs.

Scaleway:
- Aspis Bio project pinning, excluding default/non-Bio projects.
- Instance CPU/GPU inventory with guarded start, stop, reboot and delete.
- Serverless Functions and Containers inventory.
- Block volumes, snapshots and Object Storage bucket inventory/usage estimate.
- Public Instance product catalog for spawnable CPU/GPU types with per-zone availability and prices.
- Best-effort IAM inventory for policies, applications, groups and API keys.
- Best-effort project inventory for Private Networks, Public Gateways, Load Balancers, KMS keys, Managed Databases, Container Registry namespaces, Kubernetes clusters and Messaging/Queuing credentials.

## Console Map Sections

Cloudflare:
- Account, IAM, audit logs.
- Workers, Pages, routes, secrets.
- R2, D1, KV, Queues, Vectorize, Hyperdrive.
- Zones, DNS, WAF, Access, Tunnel.
- Workers AI, AI Search, AI Gateway, logs, analytics, billing.

Scaleway:
- Projects, IAM policies, applications, groups, API keys.
- CPU/GPU Instances.
- Spawnable CPU/GPU product catalog.
- Serverless Functions, Containers, Jobs.
- Object Storage, Block Storage, snapshots.
- VPC, public gateways, IPs, load balancers, security groups, KMS, audit trail.
- Managed databases, registry, messaging/queues and observability.

## Rules

- Never expose provider mutation actions outside the selected Aspis Bio scope. If an account/project display name is ambiguous, require a saved pinned id and show a warning.
- Inventory/read surfaces may be partial; destructive/write actions require separate backend guards.
- Product catalogs can be public/read-only; create flows still require project/account scoped write permissions.
- UI can show roadmap/unavailable sections, but must not pretend a token can access products that returned 403 or are enterprise-only.
- API key secret values must never be serialized back into the React UI; access-key identifiers may be shown as credential metadata.

## Official References

- Cloudflare API: https://developers.cloudflare.com/api/
- Cloudflare Workers API: https://developers.cloudflare.com/api/resources/workers/
- Cloudflare R2 API: https://developers.cloudflare.com/api/resources/r2/
- Cloudflare Zones API: https://developers.cloudflare.com/api/resources/zones/
- Cloudflare DNS API: https://developers.cloudflare.com/api/resources/dns/subresources/records/
- Cloudflare Rulesets API: https://developers.cloudflare.com/api/resources/rulesets/
- Cloudflare Audit Logs API: https://developers.cloudflare.com/api/resources/audit_logs/
- Cloudflare AI Gateway API: https://developers.cloudflare.com/api/resources/ai_gateway/
- Cloudflare Logpush API: https://developers.cloudflare.com/api/resources/logpush/
- Cloudflare developer product index: https://developers.cloudflare.com/llms.txt
- Scaleway API: https://www.scaleway.com/en/developers/api/
- Scaleway Instance API: https://www.scaleway.com/en/developers/api/instance
- Scaleway IAM API: https://www.scaleway.com/en/developers/api/iam
- Scaleway Public Gateway API: https://www.scaleway.com/en/developers/api/public-gateway/
- Scaleway Key Manager API: https://www.scaleway.com/en/developers/api/key-manager/keys
- Scaleway Managed Database API: https://www.scaleway.com/en/developers/api/managed-database-postgre-mysql/
- Scaleway Messaging and Queuing SQS API: https://www.scaleway.com/en/developers/api/messaging-and-queuing/sqs-api/
- Scaleway NATS API: https://www.scaleway.com/en/developers/api/nats/nats-api/
- Scaleway Registry API: https://www.scaleway.com/en/developers/api/registry/
- Scaleway Object Storage docs: https://www.scaleway.com/en/docs/object-storage
