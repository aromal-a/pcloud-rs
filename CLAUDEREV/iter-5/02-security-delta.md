# iter-5 02-security delta

Scope: verification-only delta after iter-4 fix campaign converged 3x.

## Verification A: credential-shaped strings in iter-4 edits

iter-4 security-scope edits touched packaging/systemd, README intro, and
C_FEATURE_PARITY_MATRIX.csv rows 81-83.

Checked:

- `packaging/**` — `password|secret|token|api_key|credential` mentions are
  documentation references (man pages describe `crypto-send-change-private`
  token aliases, README describes config keys). No literal credential
  values, no embedded passwords, no hardcoded tokens.
- `packaging/init/common/pcloudd.env.example` — env-var stubs only,
  placeholder shape (no real values).
- `README.md` — pattern `(password|token|secret|api_key)\s*=\s*"literal"`
  returns 0 matches.
- `C_FEATURE_PARITY_MATRIX.csv` — 39 hits for `password|secret|token`, all
  feature-name references (`psync_change_password`,
  `psync_crypto_send_change_user_private`, etc.). No credential literals.

No credential-shaped String introduced.

## Verification B: cargo deny check

```
advisories ok, bans ok, licenses ok, sources ok
```

Status preserved from iter-4.

## Conclusion

0 new findings. 0 retractions. 0 regressions.

delta count: 0 new, 0 retractions, 0 regressions
