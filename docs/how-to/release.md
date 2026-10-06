# Publier une release

Une release, c'est un tag `vX.Y.Z` sur `main`. Deux publications en découlent : sur
**GitLab**, la source, la CI construit l'archive Linux x86_64 (glibc), la dépose dans le
registre de paquets et crée la page de release ; sur **GitHub**, le miroir
`github.com/deepika-public/deepika-sync` reçoit le tag, et le workflow
`.github/workflows/release.yml` construit sur chaque système les archives Linux x86_64 et
arm64 (musl, statiques) et macOS Apple Silicon et Intel, les teste, puis publie une release
avec ces quatre archives, `SHA256SUMS` et leurs attestations de provenance. C'est elle que
le plugin dot-sync installe. Publier le daemon **avant** le plugin, dont le pipeline teste
l'intégration contre une archive publiée.

## Prérequis, une fois

- Un runner GitLab Docker Linux x86_64 avec accès réseau (images, paquets système,
  dépendances Cargo) ; le registre de paquets générique activé, et dans les réglages du
  **groupe**, **Packages and registries → Generic**, *Allow duplicates* désactivé.
- Les tags `v*` protégés, réservés aux responsables de release.
- **Settings → CI/CD → Job token permissions** : autoriser le projet
  `deepika-public/deepika-obsidian-toolbox/dot-sync`, dont le pipeline télécharge le daemon.
- Le miroir vers GitHub (*Settings → Repository → Mirroring repositories*, direction
  *Push*, `ssh://github.com/deepika-public/deepika-sync.git`, utilisateur `git`, clés d'hôte
  détectées, authentification par clé SSH) ; sa clé publique en *deploy key* avec accès en
  écriture du dépôt GitHub. Le miroir ne pousse qu'un tag à la fois : GitHub ne déclenche
  aucun workflow pour un push de plus de trois tags.

Aucun jeton personnel : le job utilise `CI_JOB_TOKEN`. Rien à compiler sur le poste pour
publier. Le client web, lui, n'a pas de release : il se redéploie à chaque fusion dans
`main` qui touche `web/` (jobs `web-wasm` et `pages`).

## Publier

1. Sur une branche, passer `version` dans `Cargo.toml` (et `Cargo.lock` via
   `cargo check`), ajouter l'entrée dans le
   [journal des versions](../reference/compatibility.md#journal-des-versions) et, si le
   protocole réseau change, le dire dans le tableau des contrats.
2. Merge request, pipeline vert, fusion dans `main`. **Attendre que le pipeline de `main`
   soit vert** avant de taguer : un tag posé sur un commit rouge publie un binaire non
   validé.
3. Poser le tag sur ce commit de `main`, jamais réutilisé ni déplacé :

   ```bash
   git fetch origin && git switch --detach origin/main
   git tag -a vX.Y.Z -m 'deepika-sync X.Y.Z'
   git push origin vX.Y.Z
   ```

4. Attendre le job `release`, puis vérifier sur la
   [page des releases](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/deepika-sync/-/releases) la
   présence de `deepika-sync-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` et de `SHA256SUMS`
   (« Source code » n'est pas le binaire). Télécharger l'archive et refaire
   [l'installation](install.md) sur une machine vierge : `sha256sum -c`, `--version`,
   `BUILD.json` avec le bon commit et `"dirty": false`.
5. Attendre le workflow GitHub, puis vérifier sur la
   [release GitHub](https://github.com/deepika-public/deepika-sync/releases) les quatre
   archives et `SHA256SUMS` ; sur un Mac, installer l'archive macOS et lancer une session.
6. Côté plugin, passer sa variable CI `DEEPIKA_SYNC_RELEASE_TAG` à ce tag et, si le plugin
   doit installer cette version, sa constante `DAEMON_RELEASE`, puis suivre son
   [guide de publication](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/dot-sync/-/blob/main/docs/how-to/release.md).

Le packaging refuse un tag qui diverge de `Cargo.toml` et un checkout modifié. Un échec
d'upload n'ouvre pas de release ; un upload partiel se vérifie avant relance. Une release
existante n'est jamais modifiée : en cas d'erreur, publier la version suivante.

## Construire l'archive localement

Pour inspecter ce que la CI GitLab produit, sous Linux x86_64 avec Rust 1.95+, outils C,
Python 3.11+ et `readelf` (`binutils`) :

```bash
cargo test --locked
python3 scripts/package.py
(cd dist && sha256sum -c SHA256SUMS)
```

L'archive contient le binaire, `README.md`, `docs/` et `BUILD.json` (commit, arbre propre
ou non, cible, compilateur, glibc minimale, protocoles). Rien n'est envoyé. Les
métadonnées d'archive sont stabilisées, ce qui ne promet pas des binaires identiques
d'un environnement à l'autre. Une autre plateforme se construit avec
`python3 scripts/package.py --target <cible>` sur ce système, comme le fait le workflow
GitHub (`musl-tools` pour les cibles Linux musl).

Références GitLab : [paquets génériques](https://docs.gitlab.com/user/packages/generic_packages/),
[releases CI](https://docs.gitlab.com/ci/yaml/#release).
