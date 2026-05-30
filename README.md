# srvcs-gcd

The greatest-common-divisor orchestrator of the srvcs.cloud distributed standard
library.

Its single concern: **number theory: greatest common divisor.** It owns the
*control flow* — an iterative Euclidean loop — but does no arithmetic of its own.
It asks [`srvcs-iszero`](https://github.com/srvcs/iszero) whether the divisor has
reached zero and [`srvcs-modulo`](https://github.com/srvcs/modulo) for each
`a mod b`, then folds the results until the loop terminates.

```
gcd(a, b):
    x, y = a, b
    while not iszero(y):
        r = modulo(x, y)
        x, y = y, r
    return x
```

`gcd(a, 0) == a` and `gcd(0, 0) == 0` fall out naturally: the first `iszero`
check breaks immediately when `b == 0`.

## API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/` | Service identity, concern, and dependency list |
| `POST` | `/` | Compute `gcd(a, b)` |
| `GET` | `/healthz` `/readyz` `/metrics` `/openapi.json` | srvcs service standard surface |

```sh
curl -s -X POST localhost:8080/ -H 'content-type: application/json' -d '{"a": 12, "b": 8}'
# {"a":12,"b":8,"result":4}
```

Responses:

- `200 {"a": a, "b": b, "result": n}` — evaluated.
- `422` — a dependency rejected the input, forwarded verbatim.
- `500` — the Euclidean loop did not converge within the iteration cap (a
  misbehaving dependency).
- `503` — a dependency is unavailable.

## Dependencies

- [`srvcs-modulo`](https://github.com/srvcs/modulo)
- [`srvcs-iszero`](https://github.com/srvcs/iszero)

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SRVCS_BIND_ADDR` | `0.0.0.0:8080` | Bind address |
| `SRVCS_MODULO_URL` | `http://127.0.0.1:8084` | Base URL of `srvcs-modulo` |
| `SRVCS_ISZERO_URL` | `http://127.0.0.1:8085` | Base URL of `srvcs-iszero` |
| `SRVCS_ENV` | `development` | Environment label for logs |
| `RUST_LOG` | `info,tower_http=info` | Tracing filter |

## Local checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Orchestration tests stand up *computing* mock `srvcs-modulo` and `srvcs-iszero`
services in-process — they read the request body and return the real
`a % b` / `value == 0`, so the Euclidean loop is genuinely exercised. See
[`srvcs/platform`](https://github.com/srvcs/platform) for the shared standard.

> Note: the `cargoHash` in `flake.nix` is inherited from the template and must be
> refreshed with a `nix build` before the Nix gates pass.
