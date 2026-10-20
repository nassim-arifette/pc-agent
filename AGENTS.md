# Instructions pour les agents (Claude, Codex)

Ce dépôt est développé avec Claude Code et Codex en parallèle. Avant toute tâche, lis ce fichier et
[docs/plan.md](docs/plan.md) : c'est le plan en cours et la vision produit à suivre.
[docs/archive](docs/archive/README.md) ne contient que des documents historiques (spec v0.1,
ancienne feuille de route, ancienne comparaison de modèles) : ne pas les suivre comme consignes.

## Vision produit (à suivre pour toute décision)

- **Le LLM est l'analyste.** Il choisit ses outils, raisonne, rédige la réponse et répond aux
  suivis. Les règles locales sont des indices, pas la réponse.
- **Notre valeur, ce sont les outils.** Ils doivent être complets, précis et sûrs : lecture seule
  par défaut, actions après validation locale.
- **La référence de qualité est Claude ou ChatGPT sur les mêmes situations.** On la mesure avec
  l'évaluation (chantier 7).
- **L'interface est une conversation**, avec historique : la réponse d'abord, les preuves à un clic.
- **Confidentialité par choix visible** : bascules dans la zone de saisie, actives par défaut, et
  aperçu exact de ce qui part à un clic (contrat C5), pas en aveuglant le modèle.
- **Mesurer au bon moment** : historique et enregistrement pendant le problème.
- **Pas d'admin par défaut.** L'élévation est ponctuelle, dans `pcagent-elevated`, sans réseau ni
  LLM.
- **Chaque chiffre cité est vérifié contre une mesure.** Le vérificateur signale, il n'écrit pas.
- **Pour la soumission du 30 octobre 2026**, le modèle est un NVIDIA open source (Nemotron) via
  Nebius Token Factory.

## Carte du dépôt

| Chemin | Rôle |
| --- | --- |
| `crates/shared` | types et statuts communs |
| `crates/telemetry` | collecte Windows (mémoire, CPU, disque, GPU/NVML, processus, historique) |
| `crates/diagnostics` | règles locales (findings) |
| `crates/storage` | scanner de dossiers |
| `crates/policy` | classification, contexte et plans |
| `crates/actions` | actions approuvées et journal |
| `crates/agent` | boucle LLM, outils, client Nebius, identifiants |
| `apps/cli` | CLI `pcagent` |
| `apps/gui` | app Tauri `pcagent-gui` : commandes Rust dans `src/`, interface en JavaScript sans build dans `ui/` ([docs/gui.md](docs/gui.md)) |

Les crates et apps prévus par le plan (`crates/store`, `crates/inspect`, `apps/elevated`,
`apps/eval`) suivent ce découpage.

## Construire et tester

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked -p pcagent -p pcagent-gui
```

- La toolchain est Rust 1.90.0 (`rust-toolchain.toml`). La CI tourne sur Windows et Ubuntu.
- Sur le PC de dev, C: est presque plein : définir `$env:CARGO_TARGET_DIR = "D:\pcagent-target"`
  avant de compiler.
- La QA visuelle de l'interface se fait sans compiler, avec les données simulées et les captures
  décrites dans [docs/gui.md](docs/gui.md).

## Conventions

- **Langues :**
  - code, commentaires, messages de commit, README et docs techniques en anglais ;
  - échanges avec le porteur du projet et documents de pilotage (`docs/plan.md`) en français.
- **Dépendances :**
  - épinglées à la version exacte (`=x.y.z`) dans `[workspace.dependencies]` ;
  - `Cargo.lock` est commité et les builds utilisent `--locked`.
- **`unsafe` :**
  - seulement dans les modules natifs (telemetry, storage, actions, futur inspect), chaque bloc précédé d'un commentaire `// SAFETY:` ;
  - les apps gardent `#![forbid(unsafe_code)]`.
- **Windows d'abord :** le workspace doit tout de même compiler et passer les tests sur Linux, avec
  des bouchons.
- **Interface :**
  - jamais `innerHTML` ; CSP stricte ; capacités Tauri minimales ;
  - la saisie d'une clé API dans un champ masqué est autorisée pour son enregistrement local ;
    la clé sauvegardée n'est jamais renvoyée à la page (seul l'indice existant de quatre caractères
    est permis), ni incluse dans les journaux ou les aperçus. La saisie est effacée après
    enregistrement ou fermeture de la vue.
- **Outils du LLM :** chaque outil a un schéma fermé, un statut de couverture et des tests sur
  données synthétiques. Un outil lit, il ne modifie rien ; les actions passent par
  `prepare_action_plan`.

## Sécurité et confidentialité (non négociable)

- **Secrets :**
  - ne jamais committer, afficher ni journaliser une clé API ou le contenu d'un fichier `.env` ;
  - les fichiers `.env` et `%APPDATA%\pcagent\.env` restent hors du dépôt.
- **Notes locales :** ne jamais committer `pc-agent-review.md`.
- **Envois au modèle :** ce qui quitte le PC suit le contrat C5 de `docs/plan.md`. Ne partent
  jamais :
  - le nom d'utilisateur ou le chemin du profil en clair ;
  - les lignes de commande ;
  - le contenu des fichiers ;
  - les secrets ;
  - les identifiants de la machine.
- **Services tiers :** n'y envoyer aucune donnée personnelle (adresse e-mail, nom), y compris dans
  les en-têtes HTTP.
- **Actions :** le modèle n'a aucune autorité d'exécution. Toute action passe par une proposition,
  puis un clic de validation local.
- **Admin :** la fenêtre et le LLM ne tournent jamais en admin.
- **Dossiers de build :** ne supprimer aucun dossier de `target\` créé par une autre session sans
  accord.

## Git et coordination

- **Branches :**
  - `main` est la seule branche durable ;
  - une branche par chantier (`wpN-sujet`), une PR vers `main` ;
  - la CI doit être verte avant la fusion, et la branche est supprimée après.
- **Commit et push :** seulement quand le porteur du projet le demande.
- **Contrats :** ceux de `docs/plan.md` (section 3) font l'interface entre pistes. Pour les changer,
  modifier d'abord le plan, dans une PR à part.
- **Suivi :** mettre à jour le tableau de la section 8 de `docs/plan.md` au début et à la fin d'un
  chantier.
- **Fichiers d'une autre piste :** avant d'en modifier un, vérifier qu'aucune PR ouverte ne le
  touche.
