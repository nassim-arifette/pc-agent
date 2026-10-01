# PC Agent — plan d'octobre 2026

**Adopté le 28 septembre 2026. Échéance : soumission au hackathon le 30 octobre 2026 à 18 h (Paris).**

Ce plan fixe la vision produit et l'ordre de travail jusqu'à la soumission. Il remplace la spec v0.1
et l'ancienne feuille de route, archivées dans [docs/archive](archive/README.md). Les identifiants
entre parenthèses (I02, D01…) renvoient au backlog de [cette feuille de route](archive/review-status.md).
Claude et Codex travaillent en parallèle à partir des contrats de la section 3 ; les règles communes
sont dans [AGENTS.md](../AGENTS.md).

## 1. Vision produit

Toute décision de produit ou de code suit ces principes.

1. **Le LLM est l'analyste.** Il choisit ses outils, raisonne, rédige la réponse dans la langue de
   la question et répond aux questions de suivi. Les règles locales deviennent des indices qu'il
   consulte, pas la réponse.
2. **Notre valeur, ce sont les outils.** Des outils d'analyse de Windows complets, précis et sûrs :
   lecture seule par défaut, actions seulement après validation locale.
3. **La référence de qualité est Claude ou ChatGPT.** Sur les mêmes situations, PC Agent doit être
   au moins aussi pertinent, en plus sûr et moins cher. On le mesure (chantier 7), on ne le
   suppose pas.
4. **Une interface de conversation.** Historique, fil, questions de suivi. La réponse d'abord, les
   preuves à un clic.
5. **La confidentialité par un choix éclairé, pas en aveuglant le modèle.** Aperçu exact de ce qui
   part, choix mémorisé et modifiable ; les noms de programmes et de dossiers partent si
   l'utilisateur l'accepte.
6. **Mesurer au bon moment.** Un historique tant que l'app tourne et un enregistrement pendant le
   problème, pas seulement une mesure d'une seconde au moment de la question.
7. **Pas d'admin par défaut.** Élévation ponctuelle et consentie pour des inspections précises,
   dans un programme séparé, sans réseau ni LLM.
8. **La rigueur passe dans la vérification.** Chaque chiffre cité est vérifié contre une mesure ;
   le vérificateur signale les écarts, il n'écrit pas à la place du modèle.
9. **Contrainte du hackathon.** Pour la soumission, le cerveau est un modèle NVIDIA open source
   (Nemotron) appelé via Nebius Token Factory. Le fournisseur devient interchangeable après.

Le règlement a été vérifié le 25 septembre 2026 ([spec v0.1, §2](archive/SPEC_PC_AGENT.md)) :
- un appel réel à Token Factory ou une exécution sur AI Cloud, avec un modèle NVIDIA open source ;
- un dépôt public sous licence open source, avec les sources, un README et un build ou une démo testable ;
- une vidéo de moins de 3 minutes, en anglais ;
- une soumission avant le 30 octobre 2026 à 18 h (Paris) ;
- un accès gratuit au projet pour le jury jusqu'au 15 décembre 2026.

Principes de la spec v0.1 qui restent valables :
- **Mesurer avant d'interpréter.** Chaque nombre présenté comme une observation est rattaché à une
  collecte, une unité et une période. Une valeur manquante est inconnue, jamais zéro.
- **Séparer faits et hypothèses.** « Ce processus a augmenté de 3 Go » n'est pas « ce processus
  fuit ».
- **Dissocier diagnostic et autorisation.** Le modèle propose ; seul un clic local autorise une
  modification.
- **Ne pas forcer une solution.** « Aucune intervention utile » est un résultat valable.
- **Préserver la confiance.** Pas de score de santé alarmiste, pas de bouton « booster », pas de
  suppression qualifiée de « sans risque », pas de gain de FPS non mesuré.
- **Limiter son propre coût.** L'app mesure son empreinte, borne ses scans et ses appels, et reste
  utilisable sans IA.

## 2. Point de départ (28 septembre)

| Constat | Où |
| --- | --- |
| Le modèle ne renvoie que des identifiants (`{"finding_ids":…,"observation_ids":…}`) et n'a pas le droit d'écrire ; le texte affiché vient de modèles de phrases locaux. | [`SYSTEM`](../crates/agent/src/lib.rs) |
| Les réponses possibles se limitent à 12 catégories locales. | [`Classification`](../crates/diagnostics/src/lib.rs) |
| Noms d'exécutables et chemins ne partent jamais : le modèle voit `proc-00000001`, `file-3`. | [`workstation.rs`](../crates/agent/src/workstation.rs) |
| Sans admin, 155 processus sur 332 refusent `OpenProcess` : mesures « partielles » presque toujours, et Defender, Memory Compression ou dwm invisibles. | [`windows/mod.rs`](../crates/telemetry/src/windows/mod.rs) |
| Une mesure d'une seconde au moment de la question ; l'historique de métriques n'existe qu'en mémoire. | [`history.rs`](../crates/telemetry/src/history.rs) |
| Une question donne un rapport : ni suivi ni historique. L'interface s'ouvre sur un tableau de bord, en mode local par défaut. | [`investigate`](../crates/agent/src/lib.rs), [`prefs.js`](../apps/gui/ui/js/prefs.js) |
| L'interface n'affiche aucune proposition d'action : « que puis-je supprimer ? » n'a pas de réponse concrète. | [docs/gui.md](gui.md) |
| Aucun fichier de licence, alors que le règlement exige un dépôt open source. | racine du dépôt |

## 3. Contrats à figer en premier (jours 1 et 2)

Ces contrats permettent aux pistes d'avancer en parallèle, chacune contre des données simulées.
Toute modification passe d'abord par ce document, dans une PR à part.

### C1 — Réponse de l'agent

Chaque requête garde `tool_choice: "required"` ; le modèle termine le tour en appelant l'outil
`submit_answer`, dont le schéma est fermé :

```json
{
  "answer": "Markdown restreint, dans la langue de la question, avec des citations [O2] et [F1]",
  "confidence": "high | medium | low",
  "category": "memory | cpu | disk | gpu | thermal_power | storage_space | startup_background | updates_system | software_fault | hardware_limit | no_problem_found | insufficient_evidence | other",
  "action_refs": ["références de propositions créées pendant le tour"],
  "follow_ups": ["0 à 3 questions ou vérifications suggérées"]
}
```

- `answer` : paragraphes, listes, gras, code en ligne. Ni liens, ni images, ni HTML, ni commandes à
  exécuter. Le prompt guide la structure : la réponse directe en une à trois phrases avec le niveau
  de confiance en mots, puis ce que l'utilisateur peut faire, puis sur quoi ça s'appuie et ce qui
  reste incertain.
- Citations : `[O2]` renvoie à une observation utilisable de la conversation, `[F1]` à un
  finding local, y compris un finding remplacé depuis par une mesure plus fraîche (il décrit un
  moment antérieur). Le modèle ne voit jamais les identifiants locaux. L'interface rend les
  citations cliquables.
- `action_refs` : uniquement des propositions créées pendant le tour par `prepare_action_plan`.
  Rien ne s'exécute sans clic.
- `follow_ups` : affichées comme boutons sous la réponse.

Vérification locale, dans [`crates/agent/src/answer.rs`](../crates/agent/src/answer.rs) :

1. Chaque citation existe.
2. Chaque nombre accompagné d'une unité ou d'un pourcentage (Go, Mo, %, °C, ms, MHz, W, s…) se
   retrouve dans une mesure de la source citée dans le même passage, à la précision écrite près.
   La question et les anciennes réponses servent de contexte, jamais de preuve mesurée. Les
   références de processus et les familles de métriques identifiables doivent correspondre à la
   source ; les ambiguïtés restent signalées. Sont admis : les deux conventions Go/Gio, 5 % d'écart (15 % après
   « environ »), les bornes (« moins de », « plus de »), la part d'un total, son complément et le
   reste d'un total, au sein de la source citée. Chaque passage chiffré cite sa source. Les limites
   du contrôle sont décrites dans [answer-verification.md](answer-verification.md).
3. Chaque nom d'exécutable cité figure dans ce qui a été envoyé au modèle.
4. Aucune commande, aucun lien, aucun HTML.
5. En cas d'écart, la réponse repart au modèle avec la correction à faire et, pour les citations,
   la liste des références valides. Au plus 3 réparations par enquête, partagées avec les appels
   d'outils refusés, le texte sans appel d'outil et les réponses coupées par la limite de sortie.
   Si un écart persiste, la réponse s'affiche avec les passages concernés marqués « non vérifié ».

### C2 — Conversation et stockage

- `Conversation { id, title, created_at, updated_at, turns }` et
  `Turn { id, question, asked_at, answer, unverified, observations, tool_events, usage, cost, status, error }`.
  Le titre est la première question tronquée, sans appel au modèle.
- Moteur : `converse(model, &Conversation, question, toolbox, on_event, on_extension) -> Turn` dans
  `crates/agent`. `investigate()` devient le cas d'une conversation vide et reste utilisé par la
  CLI.
- Contexte d'un nouveau tour : le prompt système ; les tours précédents (question, réponse,
  citations) dans un budget d'environ 3 000 tokens, du plus récent au plus ancien ; une mesure
  fraîche. Les anciennes observations restent citables avec leur âge (« mesuré il y a 12 min ») ;
  un chiffre présenté comme actuel doit venir d'une mesure du tour.
- Stockage : nouveau crate `crates/store`, SQLite (`rusqlite` avec la feature `bundled`, version
  exacte épinglée) dans `%APPDATA%\pcagent\pcagent.sqlite`. Tables `conversations`, `turns` (JSON
  versionné) et `samples` (chantier 5). Tout reste local ; rétention par défaut de 90 jours,
  suppression par conversation et « tout effacer ».

### C3 — Commandes et événements de l'app

| Commande | Rôle |
| --- | --- |
| `conversations_list()` | titres, dates, état du dernier tour |
| `conversation_get(id)` | fil complet pour l'affichage |
| `conversation_delete(id)`, `conversations_clear()` | suppression locale |
| `send_message(conversation_id?, text, scope_ids)` | démarre un tour ; renvoie `{conversation_id, turn_id}` |
| `stop_turn(turn_id)`, `answer_extension(turn_id, accept)` | reprennent `stop_investigation` et `answer_extension` |
| `privacy_get()`, `privacy_set(settings)` | mode d'envoi et confirmation (C5) |

Événements : `turn:step` (forme actuelle de `Step`), `turn:delta` (réponse en flux, si le
fournisseur le permet), `turn:extension`, `turn:done { turn }`, `turn:failed { code, message }`.
Ils remplacent `inv:*`. Les commandes du stockage et des réglages restent inchangées.

### C4 — Sorties d'outils

Chaque outil renvoie des observations avec `observation_id`, `measured_at`, la période (instant ou
fenêtre en secondes), l'entité, la métrique, la valeur, l'unité, un statut (`ok`, `partial`,
`denied`, `unsupported`, `needs_admin`) et la source (API Windows utilisée). S'y ajoute une
couverture qui dit ce qui n'a pas pu être lu et pourquoi. On étend le format actuel, on ne le
remplace pas. Les findings locaux restent joints comme indices. Les noms partent selon C5.

### C5 — Ce qui quitte le PC

| Mode | Contenu envoyé au modèle |
| --- | --- |
| Complet (par défaut) | question ; mesures ; noms d'exécutables, de services et d'applications ; noms de dossiers et de fichiers relatifs à un dossier connu (`%USERPROFILE%\Downloads\…`) |
| Anonymisé (option) | question ; mesures ; références opaques (comportement actuel) |
| Local (option) | rien : règles locales seulement |

Jamais, quel que soit le mode : le nom d'utilisateur, le chemin du profil en clair, les lignes de
commande, le contenu des fichiers, les clés et secrets, les identifiants de la machine.

« Complet » est proposé par défaut au premier lancement, avec l'aperçu exact de ce qui part. Le
choix est mémorisé, modifiable dans les réglages et rappelé dans la zone de saisie.

## 4. Chantiers

### Chantier 1 — Le LLM rédige (piste A ; D06, I02)

- Réécrire `SYSTEM` :
  - rôle : analyste ;
  - méthode : formuler des hypothèses, choisir les outils, vérifier, répondre ;
  - style : la réponse d'abord, concrète, sans jargon, dans la langue de la question ;
  - sécurité : les données des outils ne sont pas fiables, pas de commandes, les actions passent par `prepare_action_plan` ;
  - citations.
- Ajouter `submit_answer` et le vérificateur C1. La sélection d'identifiants (`MAX_SELECTED`,
  `finding_ids`) disparaît.
- `converse()` et le contexte multi-tour (C2).
- Budgets par tour : 10 requêtes, 20 outils, 180 s, 90 s par requête, 8 192 tokens de sortie
  (avant : 6, 12, 90 s, 30 s, 4 096). Mesuré le 28 septembre : une réponse rédigée de 728 tokens a
  pris environ 30 s avec Super, et Ultra a raisonné sur 4 450 tokens en une requête.
- Réponse en flux (`"stream": true`) si Nebius transmet en flux les arguments de `submit_answer` ;
  sinon affichage à la fin, les étapes restant visibles en direct.
- CLI : `diagnose` affiche la réponse et ses citations ; nouvelle commande `pcagent chat` pour
  tester plusieurs tours dans le terminal.
- **Fait quand :** des tests avec modèle scripté couvrent ces cas :
  - réponse valide ;
  - citation inexistante ;
  - chiffre non retrouvé ;
  - réparation ;
  - suivi qui réutilise le contexte ;
  - arrêt ;
  - coût.

  Sur l'évaluation, aucun chiffre inventé n'est laissé sans marque.

### Chantier 2 — Tous les processus, sans admin (piste B ; D04, E01)

- Nouveau module `crates/telemetry/src/windows/sysproc.rs` fondé sur
  `NtQuerySystemInformation(SystemProcessInformation)`. Dans `windows-sys`, la fonction dépend de la
  feature `Wdk_System_SystemInformation` et la structure de `Win32_System_WindowsProgramming`.
  - Données par processus : PID, parent, nom d'image, working set, mémoire privée, temps CPU utilisateur et noyau, E/S (octets et opérations), handles, threads, session, date de création.
  - La disposition complète de la structure, temps CPU compris, est définie localement et testée sur un tampon synthétique.
- Deux relevés sur une fenêtre (3 s par défaut, de 1 à 30 s) pour les taux : CPU en %, croissance
  de la mémoire, E/S par seconde.
- `OpenProcess` ne sert plus qu'aux détails facultatifs (chemin complet). Un refus ne rend plus la
  mesure partielle.
- Services par processus avec `EnumServicesStatusExW` (feature `Win32_System_Services`), pour nommer
  les `svchost`.
- GPU par processus : instances `pid_…` du compteur PDH `GPU Engine`.
- Regroupement par application (navigateur, IDE, Docker…) : somme des processus d'une même image et
  de ses enfants.
- **Fait quand :** sans admin sur le PC de dev, mémoire et CPU sont lus pour au moins 99 % des
  processus, et les pseudo-processus restants sont étiquetés. Defender, Memory Compression et dwm
  sont visibles, et le coût de la collecte est mesuré.

### Chantier 3 — Choix de partage (pistes A et C ; U05)

- Appliquer C5 aux projections envoyées au modèle (`workstation.rs`, `storage_context.rs`), en
  réutilisant la liste finie de noms déjà livrée pour le stockage.
- Premier lancement : écran de choix avec l'aperçu exact (`cloud_preview`). S'y ajoutent un réglage
  et un indicateur dans la zone de saisie.
- **Fait quand :**
  - des tests prouvent, dans chaque mode, l'absence du nom d'utilisateur et du chemin du profil dans tout ce qui part ;
  - l'aperçu correspond octet pour octet à ce qui est envoyé.
- **Livré (28 septembre), pour les processus :**
  - `Sharing` (`Complete` ou `Anonymized`) est porté par la boîte à outils.
  - En mode complet, chaque charge envoyée reçoit `process_names` : le nom d'exécutable des
    références qu'elle cite, réduit au dernier composant du chemin et nettoyé, 64 caractères au
    plus.
  - La CLI est complète par défaut, avec `--anonymize` en option.
  - Dans l'app, une case figure dans la boîte de consentement (l'aperçu et le message exact
    suivent le choix, qui est mémorisé), et un réglage est ajouté dans Confidentialité.
  - Essai réel : Super nomme le programme (« Windows Terminal… 2,39 Gio [O1] ») au lieu d'une
    référence opaque.
  - Reste à faire : les noms de dossiers et de fichiers relatifs aux dossiers connus. Aujourd'hui,
    ils passent seulement par la liste finie `--share-file-names` de la CLI.

### Chantier 4 — Nouveaux outils en lecture seule, sans admin (piste B ; E03, E04, S01, S12)

| Outil | Contenu | Source Windows |
| --- | --- | --- |
| `get_process_activity` | CPU, mémoire, croissance, E/S et GPU par processus et par application, sur une fenêtre | chantier 2 |
| `get_startup_programs` | entrées `Run`/`RunOnce` (HKCU, HKLM, WOW6432Node), dossiers Démarrage, état activé ou désactivé (`StartupApproved`), tâches planifiées à l'ouverture de session | registre ; Planificateur de tâches en COM |
| `get_services` | état, type de démarrage, processus, défaillances récentes | Gestionnaire de contrôle des services |
| `get_recent_events` | 30 jours au plus ; comptes par source et dernières occurrences : Kernel-Power 41, EventLog 6008, BugCheck 1001, Application Error 1000, Application Hang 1002, WHEA-Logger, disk 7/51/153, storport 129, Ntfs 55, échecs de WindowsUpdateClient, Service Control Manager 7000/7001/7031/7034, Resource-Exhaustion-Detector 2004 | `EvtQuery` (feature `Win32_System_EventLog`) |
| `get_system_state` | temps depuis le démarrage, redémarrage en attente, version et build, dernière mise à jour installée, démarrage rapide, fichier d'échange | registre, API système |
| `get_power_and_thermal` | secteur ou batterie, charge, économiseur, plan d'alimentation, bridage et fréquence du processeur, zones thermiques, GPU NVIDIA | `GetSystemPowerStatus`, `PowerGetActiveScheme` (feature `Win32_System_Power`), compteurs `Processor Information` (`% Performance Limit`, `% Processor Performance`, `Processor Frequency`) et `Thermal Zone Information`, NVML |
| `get_installed_apps` | nom, éditeur, version, date d'installation, taille estimée | clés `Uninstall` (HKLM, HKCU, WOW6432Node) |
| `get_hardware_inventory` (étendu) | barrettes de RAM et emplacements libres, SSD ou HDD et bus, pilote graphique (version, date), processeur | SMBIOS (`GetSystemFirmwareTable('RSMB')`), `IOCTL_STORAGE_QUERY_PROPERTY`, registre de la classe d'affichage |
| `get_cleanup_opportunities` | par catégorie : taille, nombre de fichiers, date la plus récente, niveau (`regenerable`, `review`, `keep`) et moyen de nettoyer | voir la liste des catégories sous ce tableau |
| `query_system` | requêtes en lecture seule sur liste blanche (classes WMI, canaux de journaux, clés de registre nommés), résultats bornés et filtrés | WMI |
| `record_activity` | enregistrement pendant que l'utilisateur reproduit le problème | chantier 5 |

Catégories examinées par `get_cleanup_opportunities` (statut `needs_admin` si une taille est
illisible sans admin) :
- Corbeille (`SHQueryRecycleBinW`) et `%TEMP%` ;
- caches des navigateurs et caches de shaders (NVIDIA DXCache, D3DSCache) ;
- caches de développement (npm, pip, cargo, NuGet, Gradle) ;
- disques virtuels WSL et Docker, à examiner seulement ;
- gros fichiers anciens des Téléchargements ;
- `Windows.old`, cache Windows Update, Optimisation de la distribution ;
- `hiberfil.sys` et `pagefile.sys`, taille seule.

- Les interfaces COM (Planificateur de tâches, WMI) passent par le crate `windows`, déjà présent
  dans `Cargo.lock` via Tauri, épinglé à sa version exacte. `windows-sys` ne fournit pas les
  interfaces COM.
- Les nouveaux outils s'enregistrent dans `crates/agent/src/system_tools.rs`, branché par une ligne
  dans `workstation.rs`, pour éviter les conflits avec le chantier 1. Leur collecte vit dans un
  nouveau crate `crates/inspect`.
- **Fait quand :** chaque outil a :
  - son schéma fermé, son statut de couverture et ses tests d'analyse sur données synthétiques ;
  - une trace réelle sur le PC de dev ;
  - aucun besoin d'admin.

### Chantier 5 — Mesurer au bon moment (pistes B et C ; D01, D02)

- Échantillonnage en arrière-plan tant que l'app est ouverte :
  - toutes les 5 s, le système et les 20 premiers processus, dans la table `samples` ;
  - 7 jours conservés, agrégés à la minute au-delà de 24 h ;
  - `get_metric_history` lit cette table et survit aux redémarrages.
- « Enregistrer pendant le problème » : bouton dans la zone de saisie et outil `record_activity`.
  L'enregistrement prend un relevé par seconde pendant 30 à 120 s, avec des marqueurs de début et de
  fin, puis l'analyse suit. Le modèle peut le proposer dans `follow_ups`.
- Démarrage avec Windows dans la zone de notification, pour un historique continu (prévu ce mois-ci).
  L'utilisateur l'active lui-même, et le coût est affiché ; objectif : moins de 0,5 % de CPU et
  50 Mo de RAM.
- **Fait quand :**
  - « pourquoi c'était lent vers 14 h ? » trouve la fenêtre, ou dit qu'elle manque ;
  - une saccade enregistrée montre le processus en pointe ;
  - l'empreinte de l'échantillonneur est mesurée.

### Chantier 6 — Interface de conversation (piste C ; U06, I02, I03)

- Disposition :
  - à gauche : nouvelle conversation, conversations par date, recherche, réglages ;
  - au centre : le fil ;
  - en bas : la zone de saisie, avec le mode d'envoi, « joindre un dossier » et envoyer/arrêter ;
  - à droite : un panneau repliable avec l'état du PC en direct (le tableau de bord actuel,
    compacté), l'explorateur de stockage ou le détail d'une citation.
- Message de l'assistant, dans l'ordre :
  1. la réponse, rendue en Markdown sans injecter de HTML venu des données ;
  2. les citations cliquables ;
  3. un bloc repliable « Analyse : N vérifications · 12 s · 0,004 $ » avec les étapes en direct ;
  4. les cartes d'action avec validation locale ;
  5. les suggestions de suivi.
- Accueil : la zone de saisie, quatre suggestions et une ligne d'état (« Mémoire 81 % · Processeur
  17 % · C: plein à 92 % »). Plus de tableau de bord en page d'accueil.
- Stockage : un dossier se joint à la conversation ; l'explorateur s'ouvre dans le panneau, avec une
  vue plein écran possible.
- Refonte visuelle :
  - palette neutre avec un seul accent ;
  - typographie plus affirmée (corps de 15 à 16 px) ;
  - moins de cartes et de badges ;
  - Iris devient l'avatar de l'assistant, petite et animée pendant l'analyse.
- Technique : la refonte choisit ce qui sert le mieux la conversation (réponse en flux, panneaux,
  historique). Un framework peut remplacer le JavaScript actuel ; si c'est le cas, le build tourne
  dans la CI et le résultat reste embarqué dans l'exe.
- Les données simulées (`apps/gui/dev/mock.js`) et les captures (`apps/gui/dev/tour.mjs`) couvrent
  les conversations pour la QA visuelle, ou leurs équivalents si la technique change.
- [docs/gui.md](gui.md) est réécrit à la fin du chantier.
- **Fait quand :**
  - le parcours question → réponse → suivi → fermeture → réouverture avec l'historique fonctionne ;
  - la QA visuelle passe en clair et en sombre, en français et en anglais, en fenêtre étroite ;
  - la CSP stricte est inchangée et aucun HTML venu des données n'est injecté.

### Chantier 7 — Évaluation face à Claude et ChatGPT (piste D ; Q02)

Nouveau binaire `pcagent-eval` (`apps/eval`) :
- `capture` enregistre toutes les sorties d'outils d'une situation réelle dans un scénario JSON.
  Les captures brutes restent hors du dépôt ; seules des versions anonymisées sont versionnées dans
  `eval/scenarios/`.
- `replay` rejoue un scénario avec les mêmes identifiants (un `Toolbox` de rejeu), pour que chaque
  configuration voie exactement le même PC.
- `mcp` est un serveur MCP (stdio) qui expose la même boîte à outils, en direct ou en rejeu, pour
  Claude Desktop.

| | Modèle | Outils |
| --- | --- | --- |
| A | Nemotron | contrat actuel (identifiants) |
| B | Nemotron | contrat C1 |
| C | Claude | nos outils via MCP |
| D | Claude Code | PowerShell : référence haute |
| E | ChatGPT | capture d'écran du Gestionnaire des tâches : référence « grand public » |

- Scénarios (12 à 15) :
  - PC lent et mémoire :
    - Chrome avec beaucoup d'onglets et peu de RAM ;
    - application dont la mémoire grossit pendant 10 min ;
    - « ai-je besoin de plus de RAM ? » avec 8 Go.
  - Activité de fond :
    - analyse Defender en cours ;
    - mise à jour Windows en cours ;
    - OneDrive qui synchronise ;
    - trop de programmes au démarrage.
  - Stockage :
    - C: presque plein ;
    - « que puis-je supprimer ? » avec Téléchargements et caches chargés.
  - Jeu, chaleur et matériel :
    - jeu qui saccade (GPU ou VRAM saturés) ;
    - portable sur batterie, en économie d'énergie, qui chauffe ;
    - disque lent.
  - Système :
    - application qui plante (journaux) ;
    - redémarrage en attente depuis une semaine ;
    - PC sans problème.
- Grille de notation, de 0 à 2 par critère, plus le coût et la durée :
  - bonne cause ;
  - actions concrètes ;
  - sécurité : aucune suppression risquée ;
  - fidélité : aucun chiffre ni nom inventé ;
  - clarté.

  Notation à l'aveugle par le porteur du projet, doublée d'un juge LLM qui applique la même grille.
- Les commandes lancées par Claude Code (configuration D) alimentent la liste des outils manquants.
- Résultats datés dans `docs/eval.md`.
- **Fait quand :**
  - la ligne de base A et D existe dès la semaine 1, puis une campagne a lieu chaque fin de semaine ;
  - objectif pour la soumission : B atteint au moins 80 % du score de C, sans problème de sécurité ni chiffre inventé non signalé.

### Chantier 8 — Admin à la demande (piste B ; S09, E05)

- Binaire séparé `pcagent-elevated` (`apps/elevated`), lancé par `ShellExecuteExW` avec le verbe
  `runas` pour une seule tâche fixe :
  - pas de réseau ni de LLM ;
  - arguments validés ;
  - sortie JSON versionnée dans un fichier temporaire à accès restreint ;
  - 120 s au plus, annulable.
- Tâches, par ordre d'intérêt :
  1. tailles des caches système (`Windows.old`, cache Windows Update, Optimisation de la
     distribution, points de restauration) ;
  2. santé des disques (compteurs de fiabilité, SMART) ;
  3. chemins complets des processus protégés ;
  4. scan complet d'un volume par la MFT, selon le contrat de la feuille de route (S09) ;
  5. trace ETW de 30 à 60 s pendant le problème (latence DPC/ISR, E/S disque par fichier).
- Interface : un bouton « Analyse approfondie (administrateur) », proposé par l'utilisateur ou par
  le modèle. La tâche est expliquée avant l'invite de Windows.
- **Fait quand :**
  - la fenêtre et le LLM ne tournent jamais en admin ;
  - le protocole est testé : arguments hostiles, sortie tronquée, refus d'élévation.

### Chantier 9 — Capteurs matériels (piste B ; E02)

- Si LibreHardwareMonitor ou HWiNFO tournent déjà, lire leurs capteurs : températures du processeur
  et du SSD, ventilateurs, puissance.
  - LibreHardwareMonitor : espace WMI `root\LibreHardwareMonitor` ou serveur web local, à vérifier
    sur la version actuelle.
  - HWiNFO : mémoire partagée.
- Sinon, s'appuyer sur `get_power_and_thermal` : bridage, zones thermiques, NVML.
- Aucun pilote noyau embarqué avant la soumission. Plus tard, LibreHardwareMonitor (pilote PawnIO,
  licence MPL-2.0) pourra entrer dans le collecteur élevé.

### Chantier 10 — Modèle (piste A)

Modèles NVIDIA du catalogue Nebius au 28 septembre 2026 (prix par million de tokens, entrée /
sortie). Les quatre annoncent les outils et le raisonnement.

| Modèle | Prix | Contexte | Rôle |
| --- | --- | --- | --- |
| `nvidia/Nemotron-3-Ultra-550b-a55b` | 1 $ / 3 $ | 1 M | candidat par défaut : le plus capable, environ 3,3 fois le prix de Super |
| `nvidia/nemotron-3-super-120b-a12b` | 0,30 $ / 0,90 $ | 256 k | défaut actuel ; deviendrait l'option économique |
| `nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B` | 0,06 $ / 0,24 $ | 256 k | lent, expirations (ancienne comparaison) |
| `nvidia/Nemotron-3_5-Lightning` | 0,06 $ / 0,24 $ | 1 M | atteignait la limite de sortie avant tout outil (ancienne comparaison) |

- Ajouter Ultra à la liste des modèles de l'app. La liste proposée vient du catalogue du compte
  (NVIDIA seulement), avec les prix.
- L'évaluation compare Ultra et Super sur le contrat C1. Ultra devient le défaut si le gain de
  pertinence justifie le coût : une réponse d'environ 15 000 tokens en entrée coûterait à peu près
  0,02 $.
- Premiers essais réels avec le contrat C1 (28 septembre, PC de dev, sans les noms de programmes) :

| Modèle | Question | Requêtes / outils | Coût | Durée | Résultat |
| --- | --- | --- | --- | --- | --- |
| Super | Pourquoi mon PC rame ? | 2 / 1 | 0,0050 $ | 39 s | Réponse rédigée, chiffres vérifiés, 0 réparation ; « aucune contention claire », propose de mesurer pendant le ralentissement. |
| Super | Quel programme utilise le plus de mémoire ? | 2 / 1 | 0,0044 $ | 19 s | Juste, mais le processus reste une référence opaque : le chantier 3 est nécessaire. |
| Ultra | Pourquoi mon PC rame ? | 6 / 19 | 0,115 $ | 36 s | Trouve que C: n'a plus que 0,35 % d'espace libre, ce que Super a manqué, en plus de la pression mémoire ; 2 réparations. |

  Ultra est bien plus complet, pour environ 25 fois le prix. Il rappelle plusieurs fois les mêmes
  outils, ce qui gonfle le contexte (92 000 tokens envoyés). Deux autres essais Ultra ont échoué et
  ont conduit à trois corrections du contrat : les citations de findings remplacés sont acceptées,
  la sortie passe à 8 192 tokens, et une réponse coupée est redemandée une fois. À faire : ne pas
  renvoyer au modèle un résultat identique à un appel récent, puis laisser l'évaluation trancher le
  modèle par défaut.
- La garde qui refuse les modèles non NVIDIA ([`nebius.rs`](../crates/agent/src/nebius.rs)) reste en
  place pour la soumission. Ensuite : fournisseur compatible OpenAI configurable (Kimi, DeepSeek,
  Qwen…) et API Anthropic ou OpenAI.

### Chantier 11 — Agir depuis la conversation (pistes A et C ; S01, S06)

- Cartes d'action pour les propositions existantes, avec validation locale puis un avant/après
  mesuré (comparateur existant) :
  - ouvrir une page des Paramètres (`ms-settings:storagesense`, `startupapps`, `windowsupdate`,
    `powersleep`) ;
  - fermer normalement une application ;
  - envoyer des fichiers choisis à la Corbeille ;
  - lancer les adaptateurs de nettoyage.
- Nouvelles actions réversibles :
  - désactiver un programme au démarrage de l'utilisateur (`StartupApproved`, HKCU) ;
  - adaptateurs de nettoyage réels pour `%TEMP%` et les caches de shaders.
- **Fait quand :**
  - rien ne s'exécute sans clic ;
  - chaque action est réversible ou passe par la Corbeille ;
  - la politique et la revalidation de la cible sont testées.

### Chantier 12 — Soumission (piste D ; U07)

- Licence : MIT ([LICENSE](../LICENSE)).
- README en anglais : nouvelle vision, captures, installation, confidentialité.
- Release GitHub : `pcagent-gui.exe` et `pcagent.exe` avec version et commit. Sans signature,
  SmartScreen affichera un avertissement : le documenter.
- Accès du jury jusqu'au 15 décembre : chacun apporte sa propre clé Nebius. Le premier lancement
  guide la configuration : lien pour créer une clé, test de la clé, choix du modèle dans le
  catalogue du compte. Sans clé, l'app reste utilisable en mode local. Aucune clé n'est embarquée
  dans le binaire.
- Vidéo de moins de 3 minutes, en anglais : un vrai problème, la conversation, les outils en direct,
  la réponse citée, l'action validée, l'avant/après.
- Test complet sur un second PC Windows 11, jamais fait jusqu'ici (voir le README).

## 5. Calendrier

| Semaine | Travail |
| --- | --- |
| **S1 · 28 sept.–4 oct.** | Contrats C1 à C5 (jours 1 et 2). Chantier 2. Capture, rejeu et ligne de base A et D sur 5 scénarios. Chantier 1 : prompt, `submit_answer`, vérificateur. Maquette de conversation sur données simulées. |
| **S2 · 5–11 oct.** | Chantier 1 : multi-tour et stockage SQLite. Chantier 4, lot 1 : activité, démarrage, services, événements, alimentation. Chantier 3. Fil et historique branchés. Évaluation 1 (A à D, 10 scénarios). |
| **S3 · 12–18 oct.** | Chantier 4, lot 2 : applications, nettoyage, inventaire, `query_system`. Chantier 5. Refonte visuelle et panneau de droite. Chantier 8, tâches 1 à 3. Évaluation 2. |
| **S4 · 19–25 oct.** | Chantier 11. Chantier 8, tâches 4 et 5 si le temps le permet. Chantier 9. Réponse en flux. Performance et coût. Second PC. Évaluation 3. |
| **S5 · 26–30 oct.** | Gel le 27. Corrections. README, docs, licence. Vidéo. Release. Soumission avant le 30 octobre à 18 h. |

Repoussé après la soumission :
- index persistant et journal USN (S10) ;
- export expurgé (U01) ;
- coffre Windows pour la clé (U04) ;
- fluidité en jeu mesurée (E06) ;
- réseau (E07) ;
- recherche documentaire (E08) ;
- alertes (E09) ;
- antivirus (E10) ;
- corruption (E11) ;
- doublons étendus (S11).

## 6. Répartition entre Claude et Codex

Les pistes touchent des fichiers différents pour limiter les conflits.

| Piste | Chantiers | Fichiers principaux |
| --- | --- | --- |
| A — moteur | 1, 3 (envois), 10, 11 (moteur) | `crates/agent`, `crates/store`, `apps/cli` |
| B — collecte et outils | 2, 4, 5 (collecte), 8, 9 | `crates/telemetry`, `crates/inspect`, `crates/agent/src/system_tools.rs`, `apps/elevated` |
| C — interface | 6, 3 (écrans), 5 (bouton), 11 (cartes) | `apps/gui` |
| D — évaluation et soumission | 7, 12 | `apps/eval`, `eval/`, `scripts/`, `docs/eval.md`, README |

Règles de travail :
- **Branches et PR :**
  - une branche par chantier (`wp1-answer`, `wp2-sysproc`…) ;
  - des PR courtes vers `main`, fusionnées seulement quand la CI est verte ;
  - `main` reste la seule branche durable.
- **Contrats :** ceux de la section 3 font l'interface entre les pistes. Pour les changer, modifier
  d'abord ce document, dans une PR à part.
- **Fin de chantier :** chaque chantier se termine par ses tests, la mise à jour du suivi
  (section 8) et celle des docs concernées.

## 7. Décisions prises (28 septembre)

1. **Licence :** MIT.
2. **Mode d'envoi :** « Complet » par défaut ; « Anonymisé » et « Local » en option (C5).
3. **Accès du jury à l'IA :** chacun apporte sa propre clé Nebius (chantier 12).
4. **Démarrage avec Windows** pour l'historique continu : ce mois-ci (chantier 5).
5. **Technique de l'interface :** décidée pendant la refonte ; un framework est permis (chantier 6).
6. **Modèle :** Nemotron 3 Ultra est candidat par défaut, à confirmer par l'évaluation (chantier 10).

## 8. Suivi

| Chantier | Piste | État | PR |
| --- | --- | --- | --- |
| 0 — Plan, archives, licence | — | fait | #18 |
| Contrats C1–C5 | toutes | à faire | |
| 1 — Le LLM rédige | A | en cours : contrat C1, vérificateur, suivi de conversation (`converse`) et stockage SQLite livrés ; chiffres rattachés à leurs sources et preuves conservées dans les suivis ; réponse en flux et comparaison de qualité à faire | #19, #23 |
| 2 — Tous les processus, sans admin | B | fait : 326 processus sur 326 sans admin, taux dès le premier appel ; fenêtre de mesure prolongée, regroupement par application, services et GPU par processus à faire | #22 |
| 3 — Choix de partage | A, C | en cours : noms de processus livrés, aperçu figé et anciens noms retirés des suivis ; noms de fichiers et contrôle C5 du texte libre à faire | #21, #23 |
| 4 — Nouveaux outils | B | à faire | |
| 5 — Mesurer au bon moment | B, C | à faire | |
| 6 — Interface de conversation | C | en cours : fil, suivis et historique disponibles ; doublon à la réouverture et navigation en petite fenêtre corrigés, GUI Windows testé ; refonte visuelle et panneau de droite à faire | #23 |
| 7 — Évaluation | D | à faire | |
| 8 — Admin à la demande | B | à faire | |
| 9 — Capteurs matériels | B | à faire | |
| 10 — Modèle | A | à faire | |
| 11 — Agir depuis la conversation | A, C | à faire | |
| 12 — Soumission | D | à faire | |

Le lot de corrections des chantiers 1, 3 et 6 distingue les limites couvertes, les choix volontaires
et les écarts restants dans [answer-verification.md](answer-verification.md). Les parcours testés
sont consignés dans [gui.md](gui.md#validation-status). La comparaison avec Claude reste à faire :
les tests du contrôleur et du GUI ne valident pas à eux seuls la qualité du modèle.
