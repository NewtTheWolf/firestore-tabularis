# 🚫 Read-only GitHub mirror

> **The canonical home of `firestore-tabularis` is on Codeberg:**
> **<https://codeberg.org/NewtTheWolf/firestore-tabularis>**
>
> This GitHub repository is an automated one-way push-mirror. Issues
> and pull requests opened *here* may be missed — please file them on
> Codeberg.

---

## `#GiveUpGitHub`

This project supports the [`#GiveUpGitHub`](https://giveupgithub.org/)
campaign from the Software Freedom Conservancy. Short version of why
the canonical repo lives elsewhere:

- GitHub Copilot was trained on hosted source code regardless of the
  license terms attached to that code.
- GitHub is a proprietary, Microsoft-owned forge running on top of
  free software it has never given back.
- [Codeberg](https://codeberg.org) is run by a non-profit
  ([Codeberg e. V.](https://codeberg.org/Codeberg/org)) on a fully
  open-source stack ([Forgejo](https://forgejo.org)).

The mirror exists so existing GitHub users can still find the project
and clone it — not as a place to do the actual work. It also runs the
release builds, because GitHub Actions has the macOS and Windows
runners we need for the cross-platform plugin matrix; the resulting
artefacts are pushed back to the Codeberg release page.

---

## Where to go

| What you want                | Where                                                                                              |
| ---------------------------- | -------------------------------------------------------------------------------------------------- |
| Full README, install, docs   | [codeberg.org/NewtTheWolf/firestore-tabularis](https://codeberg.org/NewtTheWolf/firestore-tabularis) |
| Download a release           | [Codeberg releases](https://codeberg.org/NewtTheWolf/firestore-tabularis/releases)                 |
| File an issue                | [Codeberg issues](https://codeberg.org/NewtTheWolf/firestore-tabularis/issues)                     |
| Open a pull request          | [Codeberg PRs](https://codeberg.org/NewtTheWolf/firestore-tabularis/pulls)                         |
| Just read the source         | either platform — same commits, same SHAs                                                          |
| Clone over SSH               | `git clone ssh://git@codeberg.org/NewtTheWolf/firestore-tabularis.git`                             |
| Clone over HTTPS             | `git clone https://codeberg.org/NewtTheWolf/firestore-tabularis.git`                               |

`main` (and tags, when published) are synchronised Codeberg → GitHub on
every push. Nothing flows back; this repository is effectively
read-only.
