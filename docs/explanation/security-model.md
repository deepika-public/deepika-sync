# Modèle de sécurité

## Frontières

Un membre possédant l'invitation peut lire et modifier toute la session, conserver
l'historique et inviter d'autres membres. Ce n'est pas un modèle de droits fins.
L'identité de transport est la clé publique Iroh ; les noms de présence affichés par
les éditeurs sont de simples étiquettes, non certifiées.

L'invitation v1 est un JSON base64url comprenant une version, un `EndpointAddr`
Iroh (clé publique + adresses) et une capacité aléatoire de 256 bits. La clé privée
Iroh et la capacité sont persistées dans `.collab/state.sqlite`, jamais synchronisées
comme fichiers. Tant que le daemon tourne, l'invitation figure aussi dans
`.collab/session.json` (mode 0600), pour que le plugin puisse l'afficher. Le répertoire a le mode 0700. Ne pas publier le code d'invitation, ni la
sortie de `deepika-sync status`, qui affiche la capacité. Les membres autorisés peuvent
retransmettre l'invitation ; arrêter le daemon ne révoque pas les copies distribuées.
Une nouvelle session nécessite une nouvelle racine et une nouvelle capacité.

Le [client web](../../web/README.md) reçoit l'invitation dans le fragment de l'adresse
(`#…`), que le navigateur n'envoie jamais au serveur qui héberge la page ; la page est
statique et ne voit aucune session. Un tel lien donne le même accès que l'invitation.

## Transport pair-à-pair

Iroh authentifie la clé publique du pair et chiffre les connexions QUIC avec TLS.
ALPN : `deepika-sync/yrs/2`.

**La capacité ne circule jamais.** Chaque côté ouvre par un `Hello { proof }` où

```text
proof = BLAKE3_keyed(clé = BLAKE3(capacité), id_émetteur ‖ id_destinataire)
```

et vérifie la preuve reçue contre l'identité TLS du pair (`Connection::remote_id`).
Liée aux deux identités, une preuve captée sur une connexion n'en ouvre aucune autre ;
un inconnu qui compose notre adresse n'apprend rien du secret. Aucun inventaire ni
document n'est envoyé avant que la preuve du pair soit vérifiée. Cela vaut pour le
manifeste qu'un invité demande avant de rejoindre (chemins et empreintes, jamais de
texte) : il exige la même preuve. Avant `deepika-sync/yrs/2` le `Hello`
portait la capacité en clair : tout pair du réseau local pouvait la recueillir.

Les trames sont bornées à 64 Mio. Par défaut le preset Iroh `Minimal` n'utilise ni
relais public ni lookup public ; `--relay` active explicitement le preset `N0`. Le
relais n'est ni dépositaire de l'historique ni source de vérité. Aucune télémétrie
applicative n'est ajoutée.

## Découverte locale

Le daemon s'annonce en mDNS sous un nom de service dérivé de la capacité
(`sp-` + 12 chiffres hex de son hash BLAKE3). Deux sessions différentes ne se voient
pas. Ce préfixe de hash ne permet pas de retrouver la capacité ; il révèle seulement
que deux machines partagent une session. Être découvert ne donne aucun droit : le
`Hello` tranche.

## WebSocket local

Le serveur y-sync n'écoute que sur `127.0.0.1`. Ce n'est pas une frontière suffisante :
une page web ouverte dans un navigateur peut viser cette adresse. Le handshake refuse
donc (HTTP 403) tout `Origin` en `http://` ou `https://`, et n'accepte que l'absence
d'`Origin` (clients natifs, `deepika-sync lsp`) ou un schéma d'application (`app://`,
`file://`). Tout processus local du même poste peut en revanche se connecter : le
WebSocket n'authentifie pas les processus locaux. Ne pas exposer ce port hors de la
machine.

## Filesystem

La racine est canonicalisée à l'ouverture et conservée comme capacité `cap-std`.
Tous les chemins de documents sont relatifs et validés : `..`, `.`, chemins
absolus, composants cachés, séparateurs vides et backslashes sont refusés.
Les symlinks observés sont refusés, y compris ceux restant dans la racine.
Les appels effectifs restent confinés même si un composant est remplacé entre
validation et ouverture.

Un chemin reçu du réseau n'est qu'une métadonnée du document : il passe par la même
validation avant toute écriture, et n'est jamais utilisé comme chemin de stockage
interne. Les fichiers cachés, notamment `.obsidian`, `.git`, `.env` et `.collab`,
sont exclus du partage. Seuls les `.md` lisibles en UTF-8 de 8 Mio maximum sont suivis.

Un administrateur local, le même UID malveillant modifiant l'état privé, ou un
montage hostile ne sont pas des adversaires confinables par un daemon exécuté sous
ce même UID.

## Limites

Les bornes de trames ne constituent pas une défense complète contre un membre
autorisé épuisant CPU, mémoire ou espace disque. L'état privé n'est pas chiffré au repos.

Un writer externe peut modifier un fichier après que le watcher l'a lu. Aucun watcher
ne peut promettre de capturer chaque état intermédiaire d'un writer non coopératif.
Pour éditer sans cette incertitude, passer par un éditeur connecté (LSP ou WebSocket).

## Stockage SQLite

Les chemins SQLite sont fixes et internes : `state.sqlite`, `state.sqlite-wal`,
`state.sqlite-shm` sous `.collab` (0700). L'ouverture canonicalise ce répertoire,
refuse les symlinks observés sur tous ces fichiers et sur `lock`, et utilise
`SQLITE_OPEN_NOFOLLOW`. SQLite utilise son VFS Unix standard, **pas** un VFS `cap-std` :
le stockage privé suppose une arborescence locale de confiance, non remplacée par un
autre utilisateur pendant l'exécution.

Le journal `.collab/daemon.log` ne contient ni texte de note, ni code d'invitation,
ni capacité. Il contient des chemins et des identifiants de documents :
voir [dépanner](../how-to/troubleshoot.md) avant de le transmettre.

Tests : `tests/security.rs` (mauvaise preuve ⇒ aucun document ; le `Hello` ne contient
pas le secret), `tests/ws.rs` (origine navigateur refusée), `tests/storage.rs`
(symlinks internes refusés), `tests/projection.rs` et `filesystem` (confinement).
