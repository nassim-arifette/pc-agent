# Suivi de la revue du 27 septembre 2026

Réévaluation de `pc-agent-review.md` sur `main`, y compris les PR #12 (propositions d’actions)
et #13 (identifiants Nebius dans le fichier utilisateur). Le document original est conservé.
Les constats de sûreté et de cohérence sont valides ; plusieurs propositions restantes sont
des évolutions de produit, pas des fonctions déjà promises par cette version.

## Statuts

- **Corrigé** : comportement modifié, avec une régression ciblée quand nécessaire.
- **Covered limitation** : limite connue, détectée ou explicitement représentée dans les résultats.
  Cela ne signifie pas que la capacité manquante est implémentée.
- **Wanted limitation** : restriction volontaire du prototype ou de son modèle d’autorisation.
- **À développer** : amélioration utile qui reste ouverte, sans la présenter comme corrigée.

## Constats de la revue

| Constat | État et comportement actuel |
|---|---|
| 1. Cleanup non lié au contenu approuvé | **Corrigé** : manifeste de tous les fichiers, identités du répertoire et du marqueur, nouvelle validation avant le premier effet. Suppression Windows par le handle vérifié. Fichier ajouté/remplacé, racine remplacée ou inventaire trop long : refus. |
| 2. `reversible_only` inversé | **Corrigé** : `true` par défaut, y compris lors de la lecture de préférences incomplètes. Le nettoyage synthétique exige un opt-in explicite. |
| 3. Deux JSON sur stdout | **Corrigé** : un seul document avec investigation, `actions` et `recovery`. Les plans non exécutés conservent leur manifeste local. |
| 4. N’importe quel outil satisfait l’inspection obligatoire | **Corrigé** : classes `Inspection`, `Context`, `Proposal` ; seules des observations utilisables issues d’une inspection réussie débloquent la conclusion. |
| 5. Observations absentes du rapport final | **Corrigé** : la conclusion sélectionne aussi des `observation_ids` matériel, stockage et historique, validés localement. Les faits sont produits depuis les mesures conservées ; le rapport expose `observation_refs`. Des observations seules ne suffisent pas à établir un diagnostic causal. |
| 6. Findings obsolètes encore sélectionnables | **Corrigé** : des mesures plus récentes et comparables remplacent les findings concernés ; les anciens restent dans `historical_findings`. Le modèle reçoit les `current_finding_ids`. |
| 7. Présélection des processus uniquement par RAM | **Corrigé** : tri avant limitation par working set, private commit, CPU, débit E/S lecture ou écriture. Les valeurs inconnues viennent en dernier. De nouveaux findings CPU/E/S et croissance persistante restent **à développer**. |
| 8. Énumération d’un gros dossier hors budget | **Corrigé** : parcours progressif, buffers bornés et estimation des frames actives. Les budgets de temps/RAM restent coopératifs et approximatifs : **covered limitation**. |
| 9. Workers détachés en fin d’enquête | **Corrigé** : annulation et jointure des jobs, également dans `Drop`. Une opération système bloquée peut retarder la jointure. |
| 10. Plans perdant leurs preuves | **Corrigé pour la traçabilité** : références locales aux observations qui précèdent la proposition. Elles documentent le contexte, sans prouver une relation causale ni la pertinence du remède. |
| 11. Cleanup partiel rapporté comme échec sans bilan | **Corrigé** : fichiers et octets effectivement supprimés sont conservés, même si un fichier suivant échoue ou si l’utilisateur annule. |
| 12. Échec du dernier écrit de journal masqué | **Corrigé** : `journal_status=error` interdit le résultat global `completed`, tout en gardant les effets déjà constatés. |
| 13. Recovery et annulation non branchés | **Corrigé** : récupération au démarrage du parcours d’actions, signalements `needs_inspection` persistants, aucun rejeu. Ctrl+C/Ctrl+Break pilote l’annulation pendant l’exécution approuvée. |
| 14. `Status::Ok` trop optimiste | **Corrigé** : `execution_status`, `coverage` (globale et par domaine/cible) et `diagnostic_support` sont indépendants et affichés dans la CLI. Les domaines non inspectés restent `not_collected`. Le `status` global reste `partial` si couverture ou appui sont incomplets, même après une exécution réussie. |
| 15. Documentation en retard | **Corrigé** : README, architecture, actions, stockage, télémétrie et validation alignés avec l’implémentation. Un registre de capacités partagé avec `doctor` reste **à développer**. |
| 16. Vérification indépendante de la nature de l’action | **Corrigé** : aucun verdict de performance après l’ouverture d’une fenêtre ; bilan des octets/espace volume pour le nettoyage ; comparaison système réservée à la fermeture normale. |
| 17. CI et corpus d’évaluation | **Corrigé pour la CI** : validation des pushes et PR sans auto-commit/push ni réécriture du commit évalué. Corpus représentatif et replay restent **à développer**. |

## Bugs supplémentaires traités

- **Sortie du scope de scan par substitution d’un lien.** L’autorisation capture l’objet avant
  canonicalisation. Le scan Windows utilise des ouvertures relatives aux handles avec refus du
  reparsing ; Unix utilise `openat`/`statat` sans suivi des liens. Le test Windows a reproduit puis
  bloqué une modification de jonction via `FILE_WRITE_ATTRIBUTES`, que les seuls flags de partage
  ne suffisaient pas à empêcher.
- **Comparaisons artificiellement renforcées.** Les références et fenêtres dupliquées ne comptent
  plus comme de nouvelles observations. Des fenêtres qui se recouvrent, des références réutilisées
  entre avant/après ou des sources/unités incompatibles ne produisent pas de verdict d’amélioration.
- **Conclusion locale trompeuse.** Un plus gros consommateur mémoire dont le support est
  insuffisant n’est plus sélectionné devant une conclusion utilisable ou l’abstention.
- **Texte de terminal non fiable.** Les chemins et erreurs affichés passent par le nettoyage des
  caractères de contrôle. Les helpers de tests Windows utilisent `CREATE_NO_WINDOW`.

## Wanted limitations

- Le modèle choisit uniquement des outils déclarés et des findings produits localement. Il ne
  fournit ni nombres inventés, ni commandes, ni autorisation d’exécution.
- Une action exige une validation locale explicite. `--json` et une entrée non interactive
  n’exécutent jamais les plans. Le cloud exige un opt-in et respecte les préférences utilisateur.
- Le seul nettoyage permanent est celui du cache synthétique marqué de pcagent : fichiers directs,
  manifeste borné, NTFS et une seule liaison par fichier. Aucun nettoyage générique d’AppData,
  de dossiers personnels ou système ; aucune élévation ni fermeture forcée.
- La télémétrie et les effets natifs ciblent Windows. Ailleurs, ils annoncent `unsupported`.
- Les scans lisent les métadonnées, pas le contenu. Les liens et placeholders reconnus sont ignorés.
  Une taille logique n’est pas une promesse d’espace récupérable ; `last_used` reste inconnu.
- Les identifiants Nebius sont lus depuis l’environnement ou `%APPDATA%\pcagent\.env`.
  Le `.env` du dossier courant est volontairement ignoré, conformément à la PR #13.

## Covered limitations

- Un échantillon court et les seuils du prototype n’établissent ni causalité, ni fuite mémoire,
  ni besoin d’achat. Un processus inaccessible reste inconnu. Le premier delta CPU/E/S est inconnu.
- Les compteurs E/S comprennent les accès mis en cache et réseau. Les compteurs disque/GPU agrégés
  ne constituent pas une attribution physique par processus. Un top N ne couvre que les processus
  lisibles et retournés, même avec un tri adapté à la ressource.
- Le scanner n’est pas un snapshot immuable. Des mutations sont détectées et rendent la couverture
  partielle ; le budget d’index n’est pas une limite absolue de mémoire du processus. Les appels
  système bloquants et le tri final peuvent dépasser le délai demandé.
- La corbeille Windows impose un passage par un chemin/PIDL après relâchement du pin. La fenêtre
  de substitution résiduelle est contrôlée puis vérifiée, pas rendue atomique ; voir
  [les garanties et limites de recycle](actions.md#recycle-binding-spec-113).
- Les octets supprimés par le cache et la variation d’espace libre du volume sont deux mesures
  distinctes : allocation, corbeille et autres processus peuvent expliquer leur différence.
- Sans identifiant de boot fiable, aucune corrélation entre boots ni fermeture d’un processus
  dont l’identité ne peut être confirmée. La fermeture normale reste hors des propositions modèle.
- Six tours peuvent être insuffisants pour un parcours stockage puis proposition. À chaque borne,
  la CLI interactive présente le bilan partiel et demande une prolongation explicite : +6 tours,
  +12 outils, +90 secondes, au maximum trois fois. Contexte, jobs et références sont conservés,
  le temps de décision humaine est exclu. JSON/non-interactif : rapport partiel avec `continuation`,
  sans prolongation automatique. La reprise après fermeture du processus reste à développer.

## Travail futur et validation

Les observations multi-échantillons, les nouveaux findings matériel/stockage/historique, le replay,
la persistance, MFT/USN, l’attribution disque/GPU, la GUI et l’installateur restent ouverts. Ils ne
sont pas considérés comme des bugs corrigés par ce lot.

La validation automatisée utilise des réponses modèle scriptées et des fixtures jetables sous
Windows et Linux. Elle ne valide pas le compte Nebius ni une session live du fournisseur. Les
tests OneDrive réels, les grands volumes, les pilotes GPU et une recette complète des actions sur
un autre PC Windows restent dans la [checklist de validation](validation.md).

Validation locale de ce lot avec Rust 1.90.0 : **214 tests réussis sous Windows**, **203 sous
Linux (WSL)** ; le test live Nebius est ignoré sur les deux plateformes. `cargo fmt --all --
--check` et `cargo clippy --workspace --all-targets --locked -- -D warnings` passent sur les deux
plateformes. Le binaire Windows compile avec `cargo build --release --locked -p pcagent`.
