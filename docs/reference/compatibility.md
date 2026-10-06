# Versions, compatibilité, plateformes

La version courante est celle de `Cargo.toml` ; `deepika-sync --version` l'affiche, et
`.collab/session.json` la publie aux clients. Cette page est **le seul endroit** de la
documentation où des numéros de version sont écrits.

## Contrats

| Contrat | Standard | Compatibilité |
| --- | --- | --- |
| Éditeurs de code | LSP 3.17 sur stdio (`deepika-sync lsp`) | Neovim, VS Code, Helix, Zed, Emacs |
| Obsidian, navigateur | WebSocket y-sync et y-awareness sur `127.0.0.1` | Tout client Yjs : `y-websocket`, `y-codemirror.next` |
| CRDT | Yrs 0.28, offsets **UTF-16** | Documents Yjs v1 ; le protocole y-sync est celui du module `yrs::sync` |
| Transport entre pairs | Iroh QUIC, ALPN **`deepika-sync/yrs/2`** | Chiffré de bout en bout ; relais public seulement avec `--relay`. Un pair d'un autre ALPN ne se connecte pas |
| Découverte locale | mDNS, service `sp-<12 hex du hash de la capacité>` | Deux sessions ne se voient pas |
| Invitation | JSON en base64url, `version: 1` | Adresse Iroh de l'hôte et capacité de la session |
| Stockage | SQLite en WAL, `<racine>/.collab/state.sqlite` | Schéma décrit dans [stockage](storage.md) ; les migrations sont automatiques à l'ouverture |
| Fichier de session | `.collab/session.json` | `version`, `pid`, `root`, `ws_port`, `invite` ; c'est par lui que le plugin et les outils trouvent la session |

**Règle.** Deux daemons se parlent tant qu'ils partagent l'ALPN ; les pairs d'une session
se mettent alors à jour l'un après l'autre. Un changement d'ALPN impose de mettre tous
les pairs à jour ensemble, et de redéployer le client web, qui parle le même protocole.
Le plugin Obsidian exige de son côté une version minimale du daemon, indiquée dans sa
propre [page de compatibilité](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/dot-sync/-/blob/main/docs/reference/compatibility.md).

## Journal des versions

Ce que chaque version change pour un utilisateur, et ce qu'elle fait à une base
existante. Le raisonnement est dans le [journal des décisions](../explanation/decisions.md).

| Version | Changements | À l'ouverture d'une base antérieure |
| --- | --- | --- |
| 0.6.0 | **Nouveau nom** : SyncParty devient deepika-sync (exécutable `deepika-sync`, variable `DEEPIKA_SYNC_LOG`, dépôt `deepika-public/deepika-obsidian-toolbox/deepika-sync`, licence MIT). **Protocole renommé** : ALPN `deepika-sync/yrs/2`, incompatible avec 0.5 ; tous les pairs d'une session doivent passer à 0.6 | Rien : le format de la base ne change pas Client web publié à `https://deepika-public.gitlab.io/deepika-obsidian-toolbox/deepika-sync`. |
| 0.5.2 | Au démarrage, le daemon rattrape ce qui a changé dans le dossier pendant son arrêt (notes supprimées, éditées, créées, déplacées) | Table `disk_state` créée ; au premier démarrage, un fichier absent est pris pour une suppression |
| 0.5.1 | Un arrivant voit tout de suite les curseurs des participants déjà présents ; `--exit-with-parent` arrête la session avec le programme qui l'a lancée | Rien |
| 0.5.0 | Protocole y-sync via `yrs`, ALPN `syncparty/yrs/2` (incompatible avec 0.4), `Hello` porteur d'une preuve et non de la capacité, `lsp` devenu client du daemon, `join` qui montre et fait accepter ce qui va changer, contrôle d'origine WebSocket, offsets UTF-16, compaction | L'index `documents` perd sa contrainte d'unicité sur le chemin ; chaque document est compacté en un instantané. Les invitations 0.4 restent valables |

## Plateformes

| Plateforme | État |
| --- | --- |
| Linux x86_64 (glibc 2.34+) | Validé : tests, harnais à deux et trois pairs, essai réel à distance par le relais. Seule cible construite par la CI |
| macOS (Apple Silicon, Intel) | **Non validé.** Rien n'est distribué ; voir les conditions ci-dessous |
| Windows | Non supporté : permissions et chemins Unix à adapter |

Ce que réussir un essai macOS demande, au-delà de la compilation :

1. **Watcher.** `notify` a un backend macOS, mais ses événements ne sont pas ceux de
   Linux ; vérifier création, sauvegarde atomique, renommage, déplacement de dossier et
   débordement. Un renommage non apparié est reconnu au texte identique ; si le texte a
   changé entre-temps, la note change d'identifiant.
2. **Chemins.** APFS existe en variantes sensibles ou non à la casse, et normalise
   parfois l'Unicode : `Note.md` / `note.md` et les formes composées / décomposées doivent
   être détectés d'après le volume réel, sans passer les noms en minuscules.
3. **Durabilité.** SQLite ne fait pas de `F_FULLFSYNC` par défaut sur macOS ; choisir et
   tester `fullfsync` avant de promettre la même garantie.
4. **Système.** Verrou de session, autorisation « réseau local » pour mDNS, chemins avec
   espaces, signature ou notarisation à la distribution.
5. **Essais.** Linux ↔ macOS et macOS ↔ macOS sur la [matrice de connexion](join-behavior.md),
   hors ligne, plantage, curseurs, collisions de noms.

## Limites communes

- Seuls les fichiers `.md` sont suivis ; texte UTF-8 jusqu'à 8 Mio ; fichiers cachés,
  binaires et liens symboliques exclus.
- Un dossier, une session ; tous les documents sont gardés en mémoire.
- Trames réseau limitées à 64 Mio.
- La récupération porte sur ce que le daemon a écrit dans SQLite : un plantage du
  processus est couvert, une coupure électrique ou un disque défaillant ne sont pas
  testés. Base active sur un système de fichiers réseau non prise en charge.
