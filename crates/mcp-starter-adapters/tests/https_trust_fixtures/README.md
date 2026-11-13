# Loopback HTTPS fixtures

These synthetic ECDSA P-256 certificates and the disposable PKCS#8 leaf key are
public test material for `https_trust.rs`. The test supplies the CA only to an
explicit adapter origin. Keep these fixtures out of platform trust stores,
production configurations, and shipped artifacts.

| Fixture | Subject alternative name | UTC validity |
| --- | --- | --- |
| `test-ca.pem` | — | 2000-01-01 to 2099-01-01 |
| `valid-localhost.pem` | `localhost` | 2020-01-01 to 2099-01-01 |
| `wrong-hostname.pem` | `wrong-name.invalid` | 2020-01-01 to 2099-01-01 |
| `expired-localhost.pem` | `localhost` | 2000-01-01 to 2001-01-01 |

The same test CA signed all three leaves with SHA-256. Each leaf uses the
public key corresponding to `test-only-leaf-key.pem` and has the TLS server
authentication extended key usage. The CA signing key was discarded after
fixture generation. The tests use the local clock for certificate validation
and require no certificate tooling at runtime.
