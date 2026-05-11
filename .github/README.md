# 🚫 Mostly read-only GitHub mirror

> **The canonical home of `firestore-tabularis` is on Codeberg:**
> **<https://codeberg.org/NewtTheWolf/firestore-tabularis>**
>
> This GitHub repository is an automated one-way push-mirror of the
> Codeberg repo. Issues and pull requests opened *here* may be missed —
> please file them on Codeberg.

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

## Why GitHub still gets used: releases

There's one pragmatic exception to the "canonical lives on Codeberg"
rule: **release downloads live on GitHub**.

The plugin ships as a cross-platform matrix (Linux x64/arm64, macOS
x64/arm64, Windows x64), and GitHub Actions is the only free CI we
have access to that provides macOS and Windows runners out of the box.
Tagging a `v*` commit on Codeberg pushes to the GitHub mirror, which
triggers the release workflow here. The resulting zips are attached
to the **GitHub Release page**, which is what the Tabularis plugin
registry — and most users — actually download from.

> If you only care about *using* the plugin, grab the latest zip from
> the [GitHub Releases page](https://github.com/NewtTheWolf/firestore-tabularis/releases).
> Everything else — source, issues, PRs — happens on Codeberg.

---

## Where to go

| What you want                | Where                                                                                              |
| ---------------------------- | -------------------------------------------------------------------------------------------------- |
| Download a release           | [GitHub releases](https://github.com/NewtTheWolf/firestore-tabularis/releases) *(built here)*      |
| Full README, install, docs   | [codeberg.org/NewtTheWolf/firestore-tabularis](https://codeberg.org/NewtTheWolf/firestore-tabularis) |
| File an issue                | [Codeberg issues](https://codeberg.org/NewtTheWolf/firestore-tabularis/issues)                     |
| Open a pull request          | [Codeberg PRs](https://codeberg.org/NewtTheWolf/firestore-tabularis/pulls)                         |
| Just read the source         | either platform — same commits, same SHAs                                                          |
| Clone over SSH               | `git clone ssh://git@codeberg.org/NewtTheWolf/firestore-tabularis.git`                             |
| Clone over HTTPS             | `git clone https://codeberg.org/NewtTheWolf/firestore-tabularis.git`                               |

`main` (and tags, when published) are synchronised Codeberg → GitHub on
every push. Source contributions flow the other direction: through
Codeberg only.
