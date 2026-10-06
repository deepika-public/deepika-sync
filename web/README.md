# deepika-sync dans le navigateur

Une page web qui rejoint une session deepika-sync **sans serveur** : un endpoint Iroh compilé en
WebAssembly, relié à l'hôte par le relais, qui parle le protocole du daemon
(`deepika-sync/yrs/2`). L'invitation est lue dans le fragment de l'URL (`#…`), que le navigateur
n'envoie à personne.

**La page est publiée** sur <https://deepika-public.gitlab.io/deepika-obsidian-toolbox/deepika-sync>, redéployée à chaque fusion
dans `main` qui touche `web/` (jobs `web-wasm` et `pages`). Elle n'a pas de version : elle
parle le protocole réseau courant du daemon, et doit être redéployée s'il change. Un lien
de session est `<page>/#<invitation>` ; le plugin Obsidian le compose (**Copy web link**).

**Ce qu'elle fait.** Rejoint la session en quelques centaines de millisecondes par le relais,
liste ses notes, ouvre celle qu'on choisit dans un éditeur CodeMirror relié par Yjs. Ce qui
est tapé dans l'onglet arrive dans le fichier de l'hôte, une modification du fichier de l'hôte
arrive dans l'onglet, accents et emoji compris ; les curseurs passent par l'awareness Yjs.
Le Rust ne fait que transporter : le cadrage et la preuve de capacité sont ceux du daemon,
et les documents sont des `Y.Doc` côté page, synchronisés avec `y-protocols`.

La liste des notes est vivante : la page tient un `Y.Doc` par note de la session, et le chemin
comme la suppression vivent dans la map `metadata` de chaque document. Une note créée, renommée,
déplacée ou supprimée ailleurs arrive donc comme une mise à jour ordinaire ; et la page crée,
renomme ou supprime une note en écrivant ces mêmes champs. À l'ouverture d'une note, elle
demande au daemon qui s'y trouve déjà (`AwarenessQuery`), pour afficher les curseurs sans attendre.

**Ce qu'elle ne fait pas** : se reconnecter seul après une coupure (recharger la page) ; renommer
un dossier d'un coup (renommer ses notes une à une) ; ménager la mémoire sur une très grosse
session, puisque toutes les notes sont chargées.

**Apparence.** Tailwind CSS 4 (`app/style.css`, compilé par `@tailwindcss/cli` dans
`dist/app.css`), thème clair ou sombre selon le système, aucune ressource tierce : ni police ni
script venus d'ailleurs, sur une page dont l'adresse porte une invitation. L'apparence de
l'éditeur lui-même est un thème CodeMirror (`SHEET` dans `app/main.js`).

## Construire

```bash
rustup target add wasm32-unknown-unknown
(cd web && cargo install wasm-bindgen-cli --locked --version "$(cargo pkgid wasm-bindgen | cut -d@ -f2)")   # celle de Cargo.lock
sudo apt install clang                                      # ring compile du C vers wasm

cd web
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/deepika_sync_web.wasm
npm ci && npm run build        # la page : CodeMirror, Yjs, y-codemirror.next
npm run serve                  # http://127.0.0.1:8765
```

Sans `clang`, Zig convient comme compilateur C : pointer `CC_wasm32_unknown_unknown` vers un
script qui retire l'argument `--target=…` et lance `zig cc -target wasm32-freestanding "$@"`,
et `AR_wasm32_unknown_unknown` vers `zig ar`.

## Essayer

L'hôte doit partager **avec le relais** : un navigateur ne peut rien joindre d'autre.

```bash
deepika-sync share /chemin/vers/dossier --relay
```

Ouvrir `http://127.0.0.1:8765/index.html#<invitation>`, cliquer une note, écrire. Ouvrir la même
adresse dans un second onglet, ou la même note dans un autre éditeur, pour voir les deux côtés.

## Limites

- **Tout passe par le relais** : pas de connexion directe depuis un navigateur. Le contenu
  reste chiffré de bout en bout ; le relais voit qui parle à qui, et le débit est le sien.
- **L'hôte doit être en ligne**, ou un autre pair de la session.
- **L'invitation donne tout** : toute la session, en écriture, sans révocation. Diffuser un
  lien largement demande un droit plus étroit (une note, lecture ou écriture, révocable), à
  concevoir dans le daemon.
- **Types dupliqués** : `WireMessage`, `ManifestEntry` et `proof` sont recopiés depuis le
  daemon. Les sortir dans une caisse commune avant d'aller plus loin, pour qu'ils ne divergent pas.
- **Taille** : 4,6 Mo de wasm non optimisé ; `wasm-opt -Oz` et `opt-level = "z"` restent à faire.
