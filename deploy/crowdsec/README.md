# CrowdSec configuration for Agro

CrowdSec runs **in front of** Agro, not inside it: Agro rate-limits its own endpoints, and the
bouncer decides who reaches them at all. These two files teach it to tell Agro's authentication
outcomes apart.

## Why they are needed

Agro's login endpoint answers `401` to three different situations, and only one is an
authentication failure:

| Stage | Means | Should a bouncer count it? |
|---|---|---|
| `credentials-rejected` | Wrong passphrase, or no such account | **Yes** |
| `second-factor-required` | A code is needed. The normal first step of *every* 2FA login | No |
| `second-factor-rejected` | Wrong or expired code, from someone whose passphrase was right | No — Agro limits this itself |

Without these files a bouncer counting `401`s counts all three. The second happens on every
successful 2FA login, so a user with 2FA enabled generates a "failed login" every time they sign
in — which is how ordinary users were getting banned mid-login.

Each response also carries the stage in an `X-Agro-Auth-Stage` header, for anything reading
responses rather than logs.

## Installing

```sh
sudo cp parsers/s01-parse/agro-auth.yaml   /etc/crowdsec/parsers/s01-parse/
sudo cp scenarios/agro-bruteforce.yaml     /etc/crowdsec/scenarios/
sudo systemctl reload crowdsec
```

Check it took:

```sh
sudo cscli scenarios list | grep agro
sudo cscli explain --log 'stage="credentials-rejected" a login was refused' --type agro
```

## Note on `X-Forwarded-For`

Agro trusts that header only from a proxy — loopback and the private ranges by default, or whatever
`AGRO_TRUSTED_PROXY` names. If CrowdSec's bouncer sits on a public address, set that variable, or
every request will be rate-limited as if it came from the proxy and one attacker will exhaust the
bucket for everybody.
