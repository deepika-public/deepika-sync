# Rejoindre une session

Ce que `join` fait d'un dossier, neuf ou déjà rempli, et pourquoi. « Nouveau dossier »
signifie sans `.collab` ; une racine qui a déjà son `.collab` est une **reprise**.

## Identité et admission

- Un UUID identifie un document. Son chemin et son état supprimé sont des métadonnées
  du CRDT (map `metadata`), modifiables et synchronisées.
- Une invitation admet un membre à toute la session, en lecture et écriture.
- `join` refuse une racine dont la capacité persistée appartient à une autre session
  (`root_already_belongs_to_another_session`). Ne pas supprimer `.collab` pour contourner ce refus.
- Ne pas copier `.collab` d'Alice chez Bob : cet état contient sa clé privée.
- À la première ouverture d'une racine, chaque `.md` présent devient un document.
  À la connexion, les deux pairs échangent un `SyncStep1` par document connu ; un
  document inconnu du destinataire est demandé en entier.

## Pourquoi deux fichiers identiques ne sont pas une seule note

Un CRDT fusionne des opérations sur des objets identifiés ; un chemin et une chaîne de
caractères ne portent pas cette identité. Deux `architecture.md` créés chacun de leur
côté sont deux documents, même au texte identique. Fusionner leurs textes reviendrait à
inventer une causalité : le résultat serait un entrelacs des deux versions. deepika-sync
tranche donc sans interaction et de la même façon partout — textes identiques, la copie
redondante est retirée ; textes différents, les deux restent côte à côte et la fusion est
un geste humain. Le choix de l'UUID le plus grand est arbitraire mais déterministe :
chaque pair arrive seul à la même décision, sans autorité centrale.

Une reprise est différente : les fichiers sont déjà liés à leurs UUID, et une
modification hors ligne est importée comme un diff minimal contre l'état connu, qui
fusionne avec celles des autres.

## Aperçu et consentement

Rejoindre modifie le dossier. Avant d'écrire quoi que ce soit, `join` se connecte à
l'hôte de l'invitation avec une identité jetable, prouve qu'il détient la capacité et
demande le **manifeste** de la session : identifiant, chemin, état supprimé et empreinte
BLAKE3 du texte de chaque document — jamais le texte. Il le compare au dossier et annonce :

| Rubrique (`--preview`) | Sens |
| --- | --- |
| `create` | Notes de la session absentes ici : fichiers créés |
| `conflict` | Même chemin, texte différent, pas d'histoire commune : les deux versions sont gardées, l'une renommée `nom (conflit <id8>).md`. Laquelle dépend d'UUID pas encore tirés : l'aperçu ne le dit pas |
| `update` | Reprise : note connue dont le texte a divergé ; le fichier reçoit la fusion |
| `rename` | Reprise : note connue que la session a renommée |
| `delete` | Reprise : note connue que la session a supprimée ; le fichier sera supprimé |
| `identical` | Déjà identiques des deux côtés : rien ne se passe |
| `share` | Notes locales que la session n'a pas : inchangées ici, envoyées à tous les pairs |

L'utilisateur accepte ou renonce. Un refus ne laisse aucune trace dans le dossier. Le plan
est une photographie : ce que les pairs modifient entre l'aperçu et la connexion n'y
figure pas. Si l'hôte est injoignable, il n'y a pas d'aperçu et `join` échoue ; `--yes`
permet de rejoindre à l'aveugle, en attendant que l'hôte revienne.

## Matrice de décision

La comparaison de contenu est exacte, sans normalisation des fins de ligne ni d'Unicode.

| Situation | Effet sur le dossier | Effet sur la session |
| --- | --- | --- |
| Document distant, chemin libre localement | Création du Markdown | UUID distant conservé |
| Même chemin, **texte identique**, deux UUID | Fichier inchangé | L'UUID le plus grand est marqué supprimé ; une seule note |
| Même chemin, **texte différent**, deux UUID | Les deux fichiers : `nom.md` et `nom (conflit <id8>).md` | L'UUID le plus grand est renommé ; aucun texte fusionné ni perdu ; `path_collision` au journal |
| Fichier local supplémentaire | Conservé | Nouveau document, diffusé aux pairs |
| Document distant supprimé | Rien à créer | Tombstone conservé |
| Document supprimé à distance, fichier local de même chemin importé | Fichier conservé | Nouveau document distinct au même chemin (un chemin supprimé est libre) |
| Reprise : document connu modifié hors ligne | Diff minimal importé puis fusion CRDT | Contributions concurrentes conservées |
| Deux renommages concurrents du même UUID | Le fichier suit la valeur retenue | La map Yrs retient une valeur, la même partout |
| Suppression concurrente à une édition | Fichier supprimé | L'édition reste dans l'état du document ; la suppression l'emporte sur l'affichage |
| Symlink ou chemin invalide reçu | Aucune écriture | Document conservé en mémoire et en base, non projeté |
| Fichier ouvert dans un éditeur (bail) | Ni écrit ni renommé tant que le bail dure | Mis à jour dans le CRDT ; projeté à la fermeture |

Seuls les `.md` UTF-8 lisibles de 8 Mio maximum sont suivis. Fichiers cachés,
symlinks, noms non UTF-8, chemins absolus et `..` sont exclus.

## Vérification

`tests/preview.rs` couvre le plan (dossier neuf, reprise, dossier identique) et vérifie
que l'obtention du manifeste n'écrit rien chez celui qui regarde et échoue sans la capacité.
`tests/convergence.rs` couvre la collision de chemin (textes différents et identiques),
la recréation d'un chemin supprimé et la fusion d'imports disque concurrents.
`scripts/test-two-peers.py` et `scripts/test-mesh.py` couvrent la connexion réelle.

Que faire des deux fichiers après un conflit : [dépanner](../how-to/troubleshoot.md).
Le tutoriel [observer une collision de chemin](../tutorials/join-existing-folder.md) le
provoque à deux daemons sur un poste. Sensibilité à la casse et Unicode selon la
plateforme : [compatibilité](compatibility.md).
