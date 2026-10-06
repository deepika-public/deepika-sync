# Journal des décisions

Chaque entrée fige un choix structurant : son contexte, la décision, ses conséquences.
Une décision n'est pas réécrite ; elle est remplacée par une entrée plus récente, et
l'entrée remplacée le dit en tête. Les entrées antérieures au passage à Yrs (0.5.0) sont
regroupées en fin de page sous **Archive** : elles décrivent un code qui n'existe plus et
ne sont gardées que comme trace du raisonnement.

Ce que chaque version change pour l'utilisateur est résumé dans le
[journal des versions](../reference/compatibility.md#journal-des-versions).

## 2026-09-21 — 0.5.2 : le dossier est rattrapé au démarrage

**Constat.** Un dossier repartagé montrait, dans la page web, une note dont le fichier
n'existait plus. Le daemon n'examinait le dossier qu'à sa toute première ouverture
(`initialized`) ; ensuite il ne connaissait que les événements du watcher. Tout ce qui arrivait
pendant son arrêt — suppression, édition, création, déplacement — était perdu : la note
supprimée restait vivante pour tous les pairs, l'édition hors ligne était écrasée par la
prochaine projection. Depuis la 0.5.1 le daemon s'arrête avec Obsidian : le cas est devenu
courant.

**Décision.** `Projector::reconcile`, avant d'ouvrir le réseau et le WebSocket (`spawn` est
devenu asynchrone et passe en premier dans `run`). Le fichier seul ne dit pas qui a bougé : le
daemon garde donc, par note, l'empreinte du texte vu en dernier dans le fichier (table
`disk_state`, écrite à chaque projection et à chaque import). Empreinte inchangée et texte
différent : la note a avancé, on réécrit. Empreinte changée : le disque a avancé, on importe.
Fichier absent et empreinte connue : suppression. Fichier absent jamais vu : note jamais écrite,
on l'écrit. Les fichiers sans note passent par `import_file`, donc par `vanished_note` : un
déplacement hors ligne garde l'identité de la note.

**Écarté.** Comparer les dates de modification : pas de date côté CRDT, et les horloges
mentent. Tout réimporter au démarrage sans empreinte : une édition d'un pair pas encore
projetée aurait été annulée par le vieux fichier, et une note jamais écrite supprimée partout.

**Limite.** Fichier modifié pendant l'arrêt **et** note avancée sans projection (il faut un
arrêt brutal entre la réception et l'écriture) : le disque l'emporte.

## 2026-09-21 — 0.5.1 : une session lancée par un programme finit avec lui

**Constat.** Le plugin Obsidian lançait le daemon détaché : fermer Obsidian laissait le dossier
partagé, sans rien à l'écran pour le rappeler — alors qu'une invitation donne accès à tout le
dossier. Un arrêt envoyé par le plugin à sa fermeture ne couvre ni le plantage ni le `kill`.

**Décision.** `share` et `join` acceptent `--exit-with-parent` : la fin de l'entrée standard vaut
`SIGTERM`. Le lanceur garde un tube ouvert ; c'est le noyau qui le ferme quand il disparaît,
quelle qu'en soit la cause. Pas de PID à surveiller (réutilisables), pas de processus gardien,
rien de propre à une plateforme. La lecture se fait dans un thread ordinaire : une lecture
bloquée de `tokio::io::stdin` retiendrait l'arrêt du runtime. Sans l'option, rien ne change.

## 2026-09-21 — 0.5.1 : le daemon retient qui est présent

**Constat.** Dans un essai Obsidian ↔ page web, les noms et curseurs des participants
n'apparaissaient pas. Mesuré : un arrivant attend jusqu'à 16 s le curseur de quelqu'un qui était
là avant lui et ne bouge pas. L'awareness Yjs ne voyage que par changements ; un serveur
y-websocket classique garde donc l'état de chacun et le remet à tout nouvel arrivant. Le daemon,
lui, relayait sans rien retenir : seul le renouvellement automatique des clients (toutes les 15 s)
finissait par combler le trou.

**Décision.** `Workspace` retient, par document, la dernière entrée d'awareness de chaque client
(`remember_presence`), en mémoire seulement. Elle est envoyée à toute connexion qui arrive
(WebSocket et pair Iroh) et en réponse à `AwarenessQuery`. Une entrée disparaît quand le client
annonce son départ (`null`) ou après 30 s de silence, le délai de péremption de y-protocols.
Le daemon n'interprète toujours pas le contenu : il compare des horloges et recopie du JSON.
Protocole inchangé (`syncparty/yrs/2`) : un pair 0.5.0 ignore la requête et reçoit des messages
qu'il savait déjà relayer.

**Écarté.** Relayer `AwarenessQuery` aux autres éditeurs pour qu'ils répondent eux-mêmes : sans
état côté daemon, mais N réponses par arrivée, et le client y-websocket n'émet pas cette requête
sur une WebSocket — Obsidian ↔ Obsidian serait resté lent.

## 2026-09-21 — 0.5.0 : un seul chemin de mutation, des briques sur étagère, quatre défauts fermés

**Contexte.** Une revue du daemon après le passage à Yrs a trouvé quatre défauts réels,
chacun reproduit avant correction :

1. `Doc::new()` compte les offsets en **octets** alors que LSP et Yjs parlent UTF-16 :
   insérer du texte après « été » faisait paniquer Yrs.
2. `set_text` supprimait tout puis réinsérait : deux imports disque concurrents
   **dupliquaient** le contenu. C'était une régression sur l'entrée du 2026-09-20
   (« le disque s'importe en plusieurs éditions »), perdue dans le refactoring.
3. `deepika-sync lsp`, `status`, `documents` ouvraient chacun un `Workspace`, donc le verrou
   exclusif : inutilisables dès que le daemon tournait. Sans daemon, le LSP ne
   synchronisait rien.
4. Le `Hello` envoyait la **capacité en clair** avant de vérifier celle du pair : tout
   poste du réseau local pouvait composer l'adresse annoncée en mDNS et recueillir le
   secret de session. Le WebSocket local n'examinait pas l'`Origin`.

S'y ajoutaient : l'awareness appliquée mais jamais relayée, la contrainte `UNIQUE` sur
`documents.path` (un chemin supprimé ne pouvait pas être recréé ; deux pairs créant le
même chemin cassaient la synchronisation), une compaction écrite mais jamais appelée, et
une projection différée par un bail qui n'était jamais rattrapée.

**Décision.**

- **Un seul chemin de mutation**, `Workspace::mutate`. Une transaction qui ne change rien
  n'est ni persistée ni annoncée : c'est ce qui éteint les échos dans un maillage avec
  cycle, et ce qui permet aux événements de **porter la mise à jour** au lieu de faire
  recalculer un diff par connexion (disparition des `peer_vectors`, `client_sv` et du
  test `diff.len() > 2`).
- Documents en `OffsetKind::Utf16` ; import disque par diff minimal (`similar`).
- `deepika-sync lsp` devient un **client WebSocket du daemon** : une réplique Yrs par tampon.
  `status` et `documents` lisent l'index en lecture seule, sans verrou.
- `Hello { proof }` : BLAKE3 keyed par la capacité, lié aux deux identités Iroh. ALPN
  `syncparty/yrs/2`. Origines `http(s)://` refusées sur le WebSocket.
- **Collision de chemin tranchée sans interaction** : l'UUID le plus grand est renommé
  `nom (conflit <id8>).md`, ou retiré si les textes sont identiques. Déterministe, donc
  identique sur chaque pair sans autorité centrale.
- La map `metadata` du CRDT est la seule source de vérité du chemin ; la table
  `documents` n'est qu'un index, réécrit dans la transaction de chaque mise à jour.
- Compaction toutes les 200 mises à jour et à l'ouverture.

**Briques remplacées.**

| Code maison ou dépendance | Remplacé par |
| --- | --- |
| Caisse `y-sync` 0.4 (abandonnée), Yrs 0.17 | Yrs 0.28 et son module `yrs::sync` |
| Boucle d'acceptation, ensemble des pairs connectés, `PeerManager` | `iroh::protocol::Router` + `ProtocolHandler` |
| `discovery.rs` sur `mdns-sd` (133 lignes) | `iroh-mdns-address-lookup`, session filtrée par nom de service |
| Rotation de journal maison | `file-rotate` derrière `tracing-appender` |
| `base64` + `hex` | `data-encoding` |
| `rustix` pour un `flock` | `std::fs::File::try_lock` |
| `iroh-base`, `bytes`, `proptest` | supprimés (inutilisés ou redondants) |

Écartés : `yrs-warp` / `yrs-axum` (un document par groupe de diffusion, versions de Yrs
figées) ; `walkdir` (ferait perdre le confinement `cap-std` du parcours).

**Conséquences.** Les pairs 0.4 et 0.5 ne se connectent pas entre eux. Une base 0.4
s'ouvre telle quelle (migration de l'index ; l'unité d'offset est une option locale de
Yrs, les mises à jour stockées se rechargent à l'identique) : voir [installer, mettre à jour](../how-to/install.md).

**Le plugin Obsidian suit.** Il abandonne lui aussi son protocole maison pour `yjs`,
`y-websocket` et `y-codemirror.next`. Trois ajouts au daemon en découlent : le fichier
`.collab/session.json` (pid, port, invitation) par lequel un éditeur trouve la session
sans analyser la sortie standard ; le marqueur `superseded_by` qui distingue un doublon
retiré d'une vraie suppression ; et un watcher qui suit les déplacements de dossiers
entiers, les renommages non appariés et les renommages différés par un bail. Le port
WebSocket est lié avant l'annonce de la session : un port occupé fait échouer le
lancement au lieu de laisser tourner un daemon sans interface éditeur.

**Rejoindre demande un accord éclairé.** Rejoindre crée, fusionne, renomme et parfois
supprime des fichiers chez l'invité, qui n'en savait rien avant. `join` obtient
désormais le manifeste de la session (chemins et empreintes, après la même preuve de
capacité), en déduit un plan et ne continue qu'après un oui — au terminal, ou dans la
fenêtre du plugin, qui passe par `--preview` puis `--yes`. Tant que l'accord n'est pas
donné, rien n'est écrit : le journal lui-même n'est ouvert qu'ensuite. Sans terminal et
sans `--yes`, la commande refuse plutôt que de supposer l'accord.

**Un pair perdu est rappelé.** Le daemon composait l'hôte de l'invitation une seule fois
et n'en gardait aucune trace : une coupure réseau de plus de quinze secondes, une mise en
veille ou un redémarrage arrêtaient la synchronisation sans retour, et un invité relancé
par `share` n'appelait plus personne. Sur un réseau local mDNS masquait le défaut ; à
distance, non. Les pairs appelés sont maintenant mémorisés et rappelés avec une pause
croissante, et toutes les tâches réseau s'arrêtent à la fermeture de l'endpoint — sans
quoi son port restait lié et bloquait le démarrage suivant.

**L'écho de ses propres écritures se reconnaît sur plusieurs écritures, pas sur la dernière.**
Le daemon ne retenait que l'empreinte de sa dernière écriture par fichier. Deux écritures
rapprochées, et l'événement en retard de la première lisait un texte qu'il ne reconnaissait
plus : pris pour une modification extérieure, réimporté, ce texte annulait le plus récent
sur tous les pairs. `scripts/test-mesh.py` échouait ainsi environ une fois sur six. Les
écritures des cinq dernières secondes sont maintenant retenues. Reste une fenêtre connue :
une modification extérieure faite dans les ~100 ms qui précèdent l'arrivée d'une mise à
jour distante est écrasée avant d'avoir été importée ; la fermer demande de garder le
dernier texte projeté comme base de fusion.

**Cette entrée remplace**, pour le daemon actuel, celles du 2026-09-20 sur l'arbitre
d'autorité à trois issues, le bail durable survivant à une connexion perdue, le port
`Files`, le second socket éditeur et les défauts liés aux curseurs Automerge : les modules
qu'elles décrivent (`ownership`, `ipc`, `compat`, `Engine`) ont disparu avec le passage à
Yrs. Un bail est désormais la connexion même ; un fichier loué n'est ni importé ni écrit,
sans avertissement. La ligne « CRDT texte : yrs écarté » du tableau du 2026-09-20 est
également caduque. Ces entrées restent ci-dessous comme trace du raisonnement d'alors.

## 2026-09-21 — Refactoring d'envergure : Adopter Yrs, LSP et WebSocket standard, supprimer le code propriétaire

**Contexte.** deepika-sync souffrait d'une triple redondance de protocoles éditeurs (`compat.rs` Teamtype, `ipc.rs` JSON-RPC propriétaire, `lsp.rs`), d'une fragmentation de l'état mémoire dans `Engine` (15 index parallèles pour pallier l'opacité d'Automerge), d'un thread dédié bloquant (`session.rs`) et d'une sur-ingénierie dans l'arbitrage d'autorité (`ownership.rs`). L'ensemble pesait 4 217 lignes dans `src/`.

**Décision.** 
1. Remplacer Automerge par **Yrs** (`yrs::Doc`, `yrs::Text`, `yrs::Map`) et supprimer `automerge` des dépendances.
2. Adopter les standards ouverts de l'industrie :
   - **LSP 3.17 (`lsp.rs`)** via `stdio` pour tous les éditeurs professionnels (Neovim, VS Code, Helix, Zed, Emacs).
   - **WebSocket `y-sync` (`ws.rs`)** pour Obsidian et les clients Web, compatible avec `@y/y-codemirror.next` et `y-websocket`.
3. Supprimer tous les protocoles propriétaires et code legacy : `compat.rs`, `ipc.rs`, `journal.rs`, `presence.rs`, `ownership.rs`, `renames.rs`, `document.rs`, `session.rs`, `crdt.rs`.
4. Remplacer le thread bloquant par un **`Workspace` Tokio asynchrone non-bloquant** (`Arc<RwLock<WorkspaceInner>>`).
5. Fusionner l'observation disque et les renommages dans un module réactif unifié [`projection.rs`](../../src/projection.rs) avec transactions SQLite ACID.
6. Adopter le protocole réseau binaire standard `y-sync` + `y-awareness` sur QUIC Iroh (`transport.rs`) couplé à la découverte locale LAN mDNS (`discovery.rs`).

**Conséquences.**
- Réduction du volume de code de 4 217 à 2 199 lignes (-48% de lignes dans `src/`, -2 018 lignes de code superflu supprimées).
- Zéro plugin requis pour les éditeurs de code grâce à LSP.
- Compatibilité native avec l'écosystème open-source mondial Yjs.
- 100% des tests validés.

---

# Archive — avant Yrs (0.5.0)

Les entrées ci-dessous datent du daemon sur Automerge, avec son IPC maison, son arbitre
d'autorité et ses baux durables. **Aucune ne décrit le daemon actuel.** Les modules,
scripts et tests qu'elles citent (`ownership`, `ipc`, `compat`, `Engine`, la commande
`release`, `scripts/test-editor-compat.py`…) ont disparu avec l'entrée « Refactoring
d'envergure » ci-dessus. Ce qui a survécu : l'import du disque en diff minimal, la
délégation de l'appariement des renommages au debouncer, et le confinement `cap-std`.

## 2026-09-20 — Le disque s'importe en plusieurs éditions, pas en une seule

*Toujours en vigueur, réimplémentée dans `projection.rs` (`set_text` applique un diff minimal).*

**Contexte.** L'import disque → CRDT (`projection.rs`) traduisait tout écart entre le fichier et
la dernière base projetée en **un seul remplacement**, obtenu en rognant le préfixe et le suffixe
communs. Ce chemin s'exécute à chaque modification externe d'un fichier et à chaque sauvegarde
automatique d'un éditeur non attaché.

Conséquence mesurée : quand deux lignes éloignées changent, tout ce qui les sépare est supprimé
puis réinséré. Automerge fusionne sans erreur, mais la contribution d'un pair éditant *à
l'intérieur* de cette plage se retrouve déplacée, car les caractères qu'elle visait n'existent
plus. Le test `distant_external_edits_preserve_a_concurrent_edit_between_them` reproduit le cas :
avant correction, l'insertion du pair atterrissait à la fin du fichier.

**Décision.** Calculer un diff de Myers ligne à ligne avec [`similar`](https://crates.io/crates/similar),
puis rogner préfixe et suffixe **à l'intérieur de chaque segment divergent** pour garder la
précision au caractère sur une modification locale. Les éditions sont produites en ordre
décroissant, parce que `Engine::edit` applique chacune contre le texte laissé par la précédente
et qu'un splice tardif ne déplace jamais un décalage antérieur.

**Conséquences.** Les positions restent en unités UTF-16 : le protocole éditeur est inchangé. Le
découpage sur des frontières de `char` ne peut pas scinder une paire de substitution. Une
modification d'un seul caractère produit une seule édition d'un seul caractère, comme avant.

## 2026-09-20 — L'appariement des renommages est délégué au debouncer

*Toujours en vigueur ; complétée en 0.5.0 par la reconnaissance d'un renommage non apparié au texte identique.*

**Contexte.** Le watcher coalesçait les rafales avec un `sleep` fixe de 100 ms non ré-armable,
suivi d'un vidage non bloquant du canal, puis d'un tri plaçant les renommages en tête du lot.
L'appariement « renommé depuis / renommé vers » reposait entièrement sur le fait que `notify`
livre lui-même un `Modify(Name(Both))` à deux chemins.

**Décision.** Adopter [`notify-debouncer-full`](https://crates.io/crates/notify-debouncer-full),
de la même équipe que `notify`, qui apparie ces événements par identité de fichier (`file-id` :
inode sur Unix, index de fichier sur Windows) et synthétise un `Modify(Name(Both))` portant les
deux chemins — exactement la forme attendue par `Engine::filesystem_event`.

**Conséquences.** Une seule caisse s'ajoute réellement à l'arbre de dépendances : `file-id`.
`notify`, `notify-types` et `walkdir` étaient déjà présents. Le délai de coalescence disparaît de
la boucle, le debouncer livrant un lot déjà réglé. Le tri des renommages est **conservé** : le
debouncer les place en tête de sa propre file, mais rien ne garantit cet ordre entre deux files
fusionnées dans un même lot, et le conserver ne coûte rien.

**Limite non résolue, et c'est important.** Le debouncer ne comprend pas le motif « écrire un
fichier temporaire puis renommer atomiquement », qu'utilisent à la fois Obsidian pour sauvegarder
et `Root::write` pour matérialiser. Cette interprétation reste à notre charge dans `projection`.
Adopter cette bibliothèque ne ferme pas ce sujet.

## 2026-09-20 — Un arbitre d'autorité explicite, à trois issues

*Remplacée par « 0.5.0 : un seul chemin de mutation » : le module `ownership` et ses verdicts n'existent plus.*

**Contexte.** L'autorité sur un fichier était un `BTreeSet<String>` de documents ouverts, passé
en paramètre à cinq sites de décision dont la sémantique divergeait : une suppression externe
était refusée, un import disque était ignoré par un `continue` nu, une matérialisation était
sautée, mais un renommage externe et l'adoption d'un fichier inconnu ne consultaient pas
l'ensemble du tout.

Le défaut de fond n'est pas l'asymétrie : c'est qu'un ensemble ne sait exprimer que deux issues.
Un `continue` silencieux est indiscernable de « rien à faire ». Une écriture tierce sur un
fichier ouvert (un `git checkout`, un `sed -i`) disparaissait donc sans trace, pour ne
ressurgir que bien plus tard sous la forme d'un `external_disk_conflict` opaque à la fermeture
du tampon.

**Décision.** Un module `ownership` à trois verdicts — `Allow`, `Defer`, `Refuse` — et une
énumération explicite des actes. Les baux sont indexés **par identité** (ce qui survit à un
renommage) **et par chemin** (ce que porte un événement du système de fichiers).

Les six décisions, chacune assumée :

| Acte | Verdict sous bail | Pourquoi |
| --- | --- | --- |
| Import disque → CRDT | `Defer` **annoncé** | L'éditeur détient le tampon ; forcer l'import l'écraserait. Mais le report est désormais signalé par `lease_contested`. |

**Ce qui compte comme divergence, et ce qui n'en est pas une.** Pendant qu'un tampon est loué,
son fichier retarde légitimement sur le document fusionné : les éditions distantes n'y sont pas
écrites. C'est l'état normal d'une session collaborative. Une première version signalait cet
état — la recette Obsidian a montré un avertissement à chaque frappe d'un collaborateur, et un
avertissement auquel on ne peut pas se fier est pire que pas d'avertissement du tout.

Le discriminant est un texte que le CRDT n'a **jamais** produit : chaque état enregistré par
l'éditeur possède un reçu (`Storage::known_text`). S'y ajoute une règle de persistance — une
divergence doit survivre un cycle pour être annoncée, car une sauvegarde automatique diverge
quelques millisecondes avant que l'édition correspondante n'arrive.
| Suppression externe | `Refuse` | Effacer est la destruction maximale. |
| Matérialisation CRDT → disque | `Defer` | Le document reste dans `projection_dirty` : cet état est la mémoire du report, non un effet de bord. |
| Effacement d'une projection obsolète | `Defer` | N'était couvert que par ricochet d'un filtre englobant. |
| Renommage externe | **`Allow`** | Un renommage déplace un document, il ne touche pas ses octets — et l'éditeur renomme lui-même les fichiers qu'il tient ouverts. Bloquer ici provoquerait un interblocage certain avec son propre déplacement. Seul l'atterrissage sur le chemin loué d'un *autre* document est refusé. |
| Adoption d'un fichier inconnu | `Refuse` si le chemin est loué | Créer une note pendant qu'on en édite une autre reste le cas nominal. |

**Conséquences.** `persist` quitte le chemin de projection au profit de `persist_projected` :
l'ancien code clonait l'état durable entier — donc chaque instantané Automerge — pour modifier
une seule ligne de la base projetée, à chaque fichier importé.

## 2026-09-20 — Le bail durable survit à une connexion perdue

*Remplacée par « 0.5.0 » : un bail est désormais la connexion même, et il n'y a plus de commande `release`.*

**Contexte.** Un bail durable restait en place après la disparition d'un client IPC, obligeant
à lancer `deepika-sync release` à la main. La correction évidente — libérer les baux à la
déconnexion, exactement comme `close_document` — a été écrite, puis **retirée**.

Le test d'intégration à deux démons l'a mise en échec : un éditeur dont la connexion tombe est
presque toujours vivant, avec des éditions non envoyées. Libérer son bail laisse le
watcher importer la copie disque, puis l'éditeur rejoue son opération à la reconnexion — et la
contribution apparaît deux fois. `docs/reference/editor-protocol.md` l'énonçait déjà :
« IPC loss MUST NOT automatically release durable document reservations ».

**Décision.** Une connexion perdue n'est pas un document fermé. Le bail est conservé ; `hello`
retourne désormais `stale_leases`, la liste des baux qu'aucun client vivant ne détient. Le
plugin rouvre d'abord ses propres documents, puis libère ce qui reste — lui seul sait quels
panneaux il possède. Aucune purge automatique côté démon : ce serait précisément l'écrasement
silencieux que ces baux existent pour empêcher.

**Conséquence.** Un Obsidian tué laisse des baux, récupérés à sa prochaine connexion. La
commande CLI `Release` demeure le recours manuel.

## 2026-09-20 — Le port `Files` oui, le port `Store` non

*Le confinement `cap-std` (`filesystem.rs`) reste ; le port `Files` en tant qu'abstraction a disparu avec `Engine`.*

**Contexte.** L'arbre de documents et la base durable étaient tous deux atteints par la même
capacité `cap-std`, et neuf endroits du code tapaient directement dans le handle brut.

**Décision.** Un trait `Files` couvrant l'arbre de documents et lui seul, derrière un
`Box<dyn Files>` dans `Engine`. `Box<dyn>` plutôt qu'un paramètre générique : un
`Engine<F: Files>` aurait contaminé `Session`, `Shared`, `transport`, `presence`, `ipc` et
`main` — une cinquantaine de signatures — pour dévirtualiser des appels dont le voisin immédiat
est un `fsync` SQLite en `synchronous=FULL` ou un aller-retour QUIC.

`.collab` reste **hors** de ce trait : c'est une capacité distincte, détenue par `Storage`, et
c'est ce qui rend la séparation possible sans toucher aux garanties testées par
`tests/security.rs`.

**`Store` n'est délibérément pas abstrait.** Un faux `Store` devrait réimplémenter l'unicité
des reçus, l'atomicité reçu + changements dans une transaction, et le WAL — c'est-à-dire tester
le faux. Or la garantie « accusé de réception durable » repose précisément sur le vrai SQLite.
Le trait viendra le jour où une seconde implémentation réelle existera.

**Conséquence.** `Engine::open_with` permet d'injecter un système de fichiers qui échoue à la
demande. Deux chemins d'erreur qui n'étaient atteints que par chance sont désormais couverts de
façon déterministe : une écriture qui échoue ne doit pas enregistrer une base de projection
jamais atteinte, et un fichier illisible mais présent est une erreur, jamais une suppression.

## 2026-09-20 — Interopérabilité éditeurs : schéma relevé, décision en attente

*Tranchée par « Refactoring d'envergure » : LSP et WebSocket y-sync, plus de socket IPC. Le script `test-editor-compat.py` cité n'existe plus.*

**Méthode.** La page de référence d'Ethersync renvoie une erreur 404. Le schéma a donc été
relevé par **observation du trafic réel** sur le socket d'un démon `teamtype` 0.9.2 installé
localement, complétée par les noms de variantes présents dans le binaire distribué. Rien n'a été
transcrit depuis leur code source, qui est sous AGPL-3.0-or-later.

**Ce qui a été établi** (et qui corrige deux hypothèses de la recherche initiale) :

| Point | Teamtype 0.9.2 | deepika-sync |
| --- | --- | --- |
| Cadrage | JSON-RPC 2.0, **une ligne par message** — pas d'en-têtes `Content-Length` | identique |
| Socket | `.teamtype/socket` | `.collab/editor.sock` |
| Messages entrants | `open {uri, content}`, `close {uri}`, `edit {uri, delta, revision}`, `cursor {uri, ranges}` | `open_document`, `close_document`, `local_edit`, `presence`, … |
| Positions | `{line, character}`, `character` en **points de code** — vérifié : un emoji compte pour 1 | décalage plat en **unités UTF-16** |
| Révision | **compteur entier** par connexion | heads Automerge |
| Ouverture | l'éditeur **déclare son contenu** ; le démon réconcilie | le démon renvoie son texte ; le plugin réconcilie |

**Bonne nouvelle.** Le cadrage est le nôtre, les messages sont peu nombreux, et la conversion
de positions — décalage UTF-16 plat ↔ `{ligne, point de code}` — est mécanique et sans perte
dans les deux sens. Ces trois points ne sont pas le problème.

**Le point dur, et il est de nature sémantique.** Leur modèle de révisions est un compteur
entier par connexion : l'éditeur annonce combien d'éditions venues du démon il a vues, et le
démon **transforme** l'édition reçue contre celles que l'éditeur ignorait. C'est une couche de
transformation d'opérations au-dessus du CRDT. Le nôtre refuse une édition positionnelle dont
la base est inconnue et impose une resynchronisation explicite — c'est ce refus qui porte la
garantie « aucun écrasement silencieux ».

De même à l'ouverture : chez eux l'éditeur déclare son tampon et le démon fait autorité ; chez
nous une divergence à l'ouverture est un conflit explicite, jamais résolu d'office.

**Décision prise : un adaptateur séparé, garantie réduite assumée.** Un second socket, à côté
de l'IPC natif qui n'est pas touché. Obsidian conserve ses garanties ; les éditeurs tiers
obtiennent la sémantique Teamtype, documentée comme telle dans
[protocoles éditeur](../reference/editor-protocol.md).

**Ce qui a rendu le coût acceptable.** Le modèle de révisions n'a finalement pas imposé de
transformation d'opérations. Leur compteur entier désigne un état ; il suffit de retenir, pour
chaque révision envoyée, les *heads* Automerge que le tampon de l'éditeur égalait alors. Une
édition entrante est appliquée sur un fork pris à ces heads puis fusionnée — ce que
`Engine::edit` fait déjà. La convergence est préservée sans autorité centrale.

**Deux écarts de comportement assumés et documentés.** Le daemon ne réécrit pas un fichier
qu'un éditeur tient ouvert, là où le leur le possède quoi qu'il arrive : l'éditeur tiers doit
enregistrer son propre tampon. Et le contenu déclaré à l'ouverture est *importé* plutôt
qu'écarté, donc rien n'est remplacé en silence même sous leur sémantique.

**Vérification.** `scripts/test-editor-compat.py` fait éditer un client ne parlant que leur
schéma et contrôle la propagation jusqu'au second pair, caractères hors BMP compris. Le relais
`teamtype client` 0.9.2 **non modifié** a été branché manuellement sur notre socket : son
édition arrive bien dans le document. Les greffons eux-mêmes n'ont pas été pilotés de bout en
bout dans leur éditeur.

## 2026-09-20 — Deux défauts trouvés en testant à la main

*Défauts du code Automerge, disparus avec lui.*

Deux comportements que la suite automatisée ne couvrait pas, relevés en manipulant deux
Obsidian réels. Les deux sont maintenant tenus par un test qui échoue sans le correctif.

**Un curseur en fin de fichier ne se propageait pas.** `map_presence` traduit une position
d'une révision à l'autre avec un curseur Automerge, qui s'ancre sur un élément de la séquence.
Or un caret se place *entre* deux caractères, donc éventuellement un cran après le dernier — et
c'est là qu'on tape, la position la plus fréquente de toutes. Automerge refusait l'index
(`index 7 is out of bounds`), `map_presence` retournait une erreur, et la présence était
silencieusement abandonnée. Correctif : utiliser `CursorPosition::End` quand la position égale
la longueur du texte à cette révision.

**Un déplacement de dossier laissait le dossier vide chez le pair.** Le pair qui n'a pas
déplacé réécrit les fichiers au nouveau chemin et efface les anciens, mais `Root::remove`
n'enlevait que le fichier. Les répertoires que nous avions nous-mêmes créés pour projeter ces
fichiers restaient, vides, et les deux arbres différaient sans raison visible. Correctif :
après une suppression, remonter les parents et les retirer tant qu'ils sont vides.
`remove_dir` refusant un répertoire non vide, une pièce jointe ou un fichier non suivi arrête
la remontée d'elle-même — ce que vérifie `a_directory_the_user_still_uses_is_never_pruned`.

**Ce que cela dit de la couverture.** Les deux défauts sont à la frontière entre le daemon et
ce que l'utilisateur voit : une position limite d'un côté, un répertoire — objet que le modèle
ne représente pas — de l'autre. Aucun n'était atteignable par les invariants testés jusque-là.

## 2026-09-20 — Ce qui reste maison, et ce qui est écarté

*Revue faite sous Automerge. Le tableau est caduc sur plusieurs lignes : `yrs` a été adopté en 0.5.0, l'IPC et le mapping Automerge n'existent plus. Gardée pour les autres refus, qui tiennent toujours (licences, projets inactifs).*

Une revue de l'écosystème a comparé une trentaine de projets aux briques du daemon. Ces refus
sont consignés pour ne pas être rouverts sans élément nouveau.

| Brique | Candidat écarté | Motif |
| --- | --- | --- |
| CRDT texte | Loro, yrs | Réécriture complète sans gain fonctionnel ; `yrs` n'offre pas d'historique persistant comparable aux heads dont dépend notre modèle de révisions |
| CRDT texte | diamond-types, cola | diamond-types ne déclare **aucune licence** ; cola n'a rien publié depuis septembre 2023 |
| Modèle de concurrence | Transformation opérationnelle | Suppose une autorité centrale, contraire au modèle pair-à-pair |
| Persistance | redb, fjall, sled | Ne résout aucun problème existant et fait perdre l'inspection manuelle de la base, qui sert de preuve d'absence d'écrasement ; `sled` est en alpha figée |
| Transport | libp2p, Veilid, p2panda, iroh-docs, iroh-willow | Régression sur le hole-punching ; `iroh-docs` est du clé-valeur last-write-wins, pas un CRDT texte ; `iroh-willow` est dormant |
| Mapping Automerge | autosurgeon | Gain nul sur un schéma à trois champs |
| Cadrage IPC | jsonrpsee, tower-lsp | Pas de transport socket Unix prêt à l'emploi ; `tower-lsp` est abandonné depuis août 2024 |
| Curseurs distants | `@codemirror/collab` | Suppose explicitement une autorité centrale construisant l'historique |
| Pont CRDT ↔ CodeMirror | `@automerge/automerge-codemirror` | Imposerait Automerge en WASM dans le plugin et détruirait l'avantage « tout le CRDT dans le daemon » |
| Contrôle d'accès | keyhive_core | Preview sans audit de sécurité ; le chiffrement de transport d'iroh couvre le modèle actuel |

**En veille, sans action.** [`samod`](https://github.com/alexjg/samod), successeur Rust
d'automerge-repo, et [`subduction`](https://github.com/inkandswitch/subduction) d'Ink & Switch
sont les deux seuls candidats qui remplaceraient réellement le protocole de synchronisation
maison en conservant Automerge et iroh. Leurs README avertissent respectivement « don't use this
anywhere serious yet » et « DO NOT use for production use cases at this time ». À réévaluer quand
ces avertissements tombent.

**Sans équivalent public, donc maison par constat et non par préférence** : la réconciliation
disque ↔ CRDT, les curseurs distants CodeMirror indépendants du CRDT, la présence, et le schéma
de messages de l'IPC éditeur.
