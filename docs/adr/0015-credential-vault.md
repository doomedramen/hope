# ADR-0015: External-key encrypted credential vault

## Status

Accepted for Milestone 6.

## Context

SSH installation needs operator-selected credentials, but the PostgreSQL
database is not a suitable place to keep plaintext secrets. Database backups
must remain useful without making a stolen backup sufficient to authenticate
to every managed host.

## Decision

Credential payloads are encrypted with ChaCha20-Poly1305 before insertion.
Each ciphertext contains a format version and a fresh random nonce. The
credential UUID and kind are authenticated as associated data, preventing a
ciphertext from being copied to another row or interpreted as another type.

The 32-byte master key is supplied outside PostgreSQL through exactly one of:

- `HOPE_CREDENTIAL_MASTER_KEY`, as 64-character hex or base64; or
- `HOPE_CREDENTIAL_MASTER_KEY_FILE`, pointing to a mounted secret file.

There is no development fallback. Missing or invalid key material fails
closed. Secret-bearing types have redacted debug output and are never included
in metadata responses, audit details, or normal error messages. A worker may
decrypt only for the duration of the operation that uses the credential.

Credential records are soft-deleted/revoked so audit history remains useful.
The key is not rotated in place yet: rotation requires decrypting and
re-encrypting every active row in one controlled maintenance operation, with
old-key availability until the migration is verified. That operation must be
added before changing the key in production.

Backing up only PostgreSQL cannot restore credential use. The master key must
be backed up through the operator's secret-management process and restored
before a database restore is used. Without it, ciphertext remains
cryptographically unrecoverable; inventory and non-secret history are still
restorable.

## Consequences

- Compromise of the database alone does not reveal SSH credentials.
- Compromise of the running server and its external key can still reveal a
  credential while a worker is actively using it; host access and key storage
  therefore remain in the deployment threat model.
- API code must use metadata types and must not add a route that returns
  decrypted payloads.
- The vault currently covers SSH key/password credentials; other credential
  types can reuse the envelope and scope model later.
