# Aspis workspace download Worker

Serves the encrypted `.aspiswspkg` bootstrap package from an R2 bucket so a
collaborator who does **not** have the Aspis Bio folder yet can pull it from
inside Aspis Management (Workspace → **Download from cloud**).

## Security model

This Worker is **transport only**. The package is already AES‑256‑GCM encrypted,
Ed25519 signed, and its data key is wrapped solely for approved device
fingerprints by the desktop app *before* upload. The Worker — and R2 — only ever
hold ciphertext. The desktop app still runs the full signature‑verified decrypt
on the downloaded bytes.

`DOWNLOAD_TOKEN` is **defense‑in‑depth** (it blocks casual access and bucket
enumeration), not the confidentiality boundary. Even a leaked URL only exposes
ciphertext that no unapproved device can decrypt.

## One‑time setup

```sh
# 1. Create the bucket (skip if it already exists).
wrangler r2 bucket create aspis-bio-workspace

# 2. Set the download token (paste a long random value).
openssl rand -base64 32          # generate one
wrangler secret put DOWNLOAD_TOKEN

# 3. Deploy the Worker.
wrangler deploy
```

## Publishing a package

```sh
# Create the package in the app (Workspace → Create bootstrap package); it lands
# under _workspace/packages/<name>.aspiswspkg. Then upload it:
wrangler r2 object put aspis-bio-workspace/aspis-bootstrap-2026-05-30.aspiswspkg \
  --file "C:/path/to/_workspace/packages/aspis-bootstrap-2026-05-30.aspiswspkg"
```

## Sharing the link

Give the collaborator a URL of the form:

```
https://aspis-workspace-download.<your-subdomain>.workers.dev/aspis-bootstrap-2026-05-30.aspiswspkg?t=<DOWNLOAD_TOKEN>
```

They paste it into **Workspace → Download from cloud**. The app downloads the
ciphertext (https only, no redirects, 1 GiB cap) and the existing decrypt then
verifies the signature, the recipient fingerprint and the signed manifest.

## Notes

- Only `GET`/`HEAD` are allowed; the key must be a single `*.aspiswspkg` segment
  (no traversal, no listing).
- Range requests are supported (206), so a large download resumes cleanly.
- The token may also be sent as `Authorization: Bearer <token>` instead of `?t=`.
- For stronger access control you can put Cloudflare Access in front of the
  Worker, but that requires an interactive login the desktop downloader cannot
  perform — the in‑URL/header token is what the app uses.
