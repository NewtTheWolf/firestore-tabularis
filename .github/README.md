# 🚫 Read-only GitHub mirror

> **The canonical home of `firestore-tabularis` is on Codeberg:**
> **<https://codeberg.org/NewtTheWolf/firestore-tabularis>**
>
> This GitHub repository is an automated one-way push-mirror of the
> Codeberg repo. Issues, pull requests, *and* release downloads all
> live on Codeberg.

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
and clone it — not as a place to do the actual work.

---

## Why GitHub is still in the loop: build runners

There's one pragmatic exception to the "everything on Codeberg" rule:
**the release builds run on GitHub Actions**.

The plugin ships as a cross-platform matrix (Linux x64/arm64, macOS
x64/arm64, Windows x64), and GitHub Actions is the only free CI we
have access to that provides macOS *and* Windows runners out of the
box. Codeberg's shared Forgejo runners are Linux-only.

The workflow here is purely a build pipeline:

1. Tag `v*` is pushed to Codeberg → mirrored to GitHub → triggers the
   workflow here.
2. GitHub Actions cross-compiles the five platform zips in parallel.
3. The final job pushes those zips back to Codeberg as release assets
   via the Forgejo API.

No release is ever published *on* GitHub. The artefacts only exist
here as ephemeral CI build outputs.

---

## Where to go

| What you want                | Where                                                                                                  |
| ---------------------------- | ------------------------------------------------------------------------------------------------------ |
| Download a release           | [Codeberg releases](https://codeberg.org/NewtTheWolf/firestore-tabularis/releases)                     |
| Full README, install, docs   | [codeberg.org/NewtTheWolf/firestore-tabularis](https://codeberg.org/NewtTheWolf/firestore-tabularis)   |
| File an issue                | [Codeberg issues](https://codeberg.org/NewtTheWolf/firestore-tabularis/issues)                         |
| Open a pull request          | [Codeberg PRs](https://codeberg.org/NewtTheWolf/firestore-tabularis/pulls)                             |
| Just read the source         | either platform — same commits, same SHAs                                                              |
| Clone over SSH               | `git clone ssh://git@codeberg.org/NewtTheWolf/firestore-tabularis.git`                                 |
| Clone over HTTPS             | `git clone https://codeberg.org/NewtTheWolf/firestore-tabularis.git`                                   |

`main` (and tags, when published) are synchronised Codeberg → GitHub on
every push. Nothing flows back; this repository is effectively
read-only for everything except the CI workflow.
