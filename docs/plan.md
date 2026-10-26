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
5. **La confidentialité par un choix visible, pas en aveuglant le modèle.** Ce qui part se règle
   par des bascules dans la zone de saisie, actives par défaut, et l'aperçu exact reste à un clic ;
   le choix est mémorisé et modifiable. Les noms de programmes et de dossiers partent tant que
   leur bascule est active (décision du 9 octobre, C5).
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

Par défaut, chaque requête garde `tool_choice: "required"` ; le modèle termine le tour en appelant
l'outil `submit_answer`.

**Exception limitée approuvée le 19 octobre 2026 (essai #85) :** après une tentative structurée de
`submit_answer` rejetée, et seulement si une inspection a déjà fourni des mesures utilisables
(ou si « Expliquer » dispose de sa capture figée utilisable), les requêtes de correction peuvent
imposer ce seul outil :
`tool_choice: {"type":"function","function":{"name":"submit_answer"}}`.
La première requête et les inspections gardent `required` ; un simple message texte, une inspection
échouée ou une demande d'autorisation administrateur ne suffisent pas à activer l'exception.
Une sortie inattendue ne donne aucune autorisation : les autres outils demandés pendant cette
correction ne sont pas exécutés. Les doublons, les lots mixtes et les réponses invalides restent
refusés, avec les mêmes plafonds de réparations, de temps et de coût. L'exception ne donne aucun droit
administrateur et ne change pas les vérifications locales. Elle ne force pas la première conclusion
après une inspection : le modèle peut toujours chercher d'autres mesures avant de tenter sa réponse.

Le schéma de `submit_answer` reste fermé :

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
- `action_refs` : uniquement des propositions créées pendant le tour par `prepare_action_plan` ou,
  pour une analyse approfondie, par `propose_elevated_check` (chantier 8). Rien ne s'exécute sans
  clic.
- `follow_ups` : affichées comme boutons sous la réponse.
- Sources web (chantier 15) : citations `[W1]` et vérification décrites en C6.

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
5. En cas d'écart, la réponse repart au modèle avec la correction à faire et, pour les chiffres
   et les citations, la liste des références valides. Une valeur juste tirée d'une mesure
   antérieure mais écrite au présent est signalée à part (`historical_wording`) : il suffit de la
   dater. En Expliquer, où tout le tour décrit une capture affichée avec son heure, ce cas est
   seulement marqué, sans réparation. La forme est lue avec souplesse et le contenu strictement :
   seul le texte de la réponse est obligatoire, un champ en trop est ignoré, une liste envoyée en
   un seul texte est découpée par ligne, une confiance ou une catégorie inconnue devient `medium`
   ou `other`, et une référence d'action inconnue est retirée et marquée. Des appels d'outils
   terminés (arguments JSON complets) sont gardés même si la réponse est ensuite coupée par la
   limite de sortie. Au plus 3 réparations par enquête, partagées avec les appels d'outils refusés, le texte
   sans appel d'outil et les réponses coupées par la limite de sortie. Une réponse coupée est
   redemandée une fois, plus courte ; une deuxième coupure de suite termine le tour. Si le modèle
   renvoie deux fois de suite exactement les mêmes écarts, les réparations s'arrêtent ;
   l'escalade du chantier 14 reste liée aux 3 réparations épuisées. Une soumission mal formée est
   décrite par une classe fermée et un champ déclaré, jamais par le texte du modèle. Si un écart
   persiste, la meilleure tentative écrite depuis les dernières mesures (le moins de passages non
   vérifiés) s'affiche, avec les passages concernés marqués « non vérifié ».

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
- Surveillances : leurs tables (migration 6) et leur rétention sont décrites en C7.

### C3 — Commandes et événements de l'app

| Commande | Rôle |
| --- | --- |
| `conversations_list()` | titres, dates, état du dernier tour |
| `conversation_get(id)` | fil complet pour l'affichage |
| `conversation_delete(id)`, `conversations_clear()` | suppression locale |
| `send_message(conversation_id?, text, scope_ids)` | démarre un tour ; renvoie `{conversation_id, turn_id}` |
| `stop_turn(turn_id)`, `answer_extension(turn_id, accept)` | reprennent `stop_investigation` et `answer_extension` |
| `current_turn()` | le tour en cours, ou rien : `{turn_id, conversation_id, question, started_at_unix_ms, stopping, extension}` ; la fenêtre le rattache après un rechargement (arrêt et demande d'extension en attente compris) |
| `privacy_get()`, `privacy_set(settings)` | mode d'envoi et bascules de la zone de saisie (C5, C6 ; complément du 9 octobre ci-dessous) |

Événements : `turn:step` (forme actuelle de `Step`), `turn:delta` (réponse en flux, si le
fournisseur le permet), `turn:extension`, `turn:done { turn }`, `turn:failed { code, message }`.
Ils remplacent `inv:*`. Les commandes existantes du stockage et des réglages restent inchangées ;
les compléments du 6 octobre ci-dessous leur ajoutent des lectures pour les vues Stockage et
Historique. Le réglage « recherche web » et la clé Tavily sont décrits en C6, les commandes et
événements des surveillances en C7.

#### Compléments backend du 4 octobre : sélection et routage

Ces ajouts préparent les commandes Rust de la phase 2 ; ils ne modifient ni les commandes
existantes, ni C1, ni les autorisations d'action. La configuration des clés et les comparaisons
avec Sonnet/Codex sont exclues de ce lot, à la demande du porteur du projet.

| Commande ajoutée | Rôle |
| --- | --- |
| `selection_sources()` | références locales opaques d'éléments déjà mesurés : ressource (dont la batterie), processus, événement, enregistrement ou élément d'un scan autorisé |
| `selection_preview(reference, intent, question?, conversation_id?, names)` | aperçu exact C5 ; sélection, intention et observations figées ; renvoie un ticket à usage unique |
| `selection_send(reference, intent, question?, conversation_id?, preview_id)` | consomme l'aperçu correspondant puis démarre le tour avec les identifiants et événements C3 existants |
| `model_routing_get()`, `model_routing_set(settings)` | mode configuré, toujours Super, toujours Ultra ou automatique ; paramètres locaux fermés, sans clé ni adresse de fournisseur |

- `intent` vaut `explain` ou `investigate`. Expliquer expose au LLM seulement les mesures figées :
  elles arrivent dans le premier message (`initial_observation`, citée `O0`) et comptent comme
  l'inspection du tour, si bien que le modèle peut répondre dès la première requête ;
  `get_selected_evidence` les relit (réponse courte `same_result_as` `O0`). Aucun collecteur,
  enregistrement ni proposition d'action n'y est disponible. Enquêter ajoute ces mesures au contexte de l'enquête normale et garde sa politique
  d'action. Le LLM rédige ; le vérificateur C1 contrôle les preuves effectivement reçues.
- Les références se résolvent exclusivement dans l'état local du moteur, jamais dans des objets
  ou des chemins fournis par la page. Un nouveau relevé crée une autre source, sans réécrire la
  sélection précédente. Les sources figées et tickets expirent après cinq minutes monotones,
  avec une capacité bornée. Une suppression d'historique, révocation de scan ou identité remplacée
  invalide les sources concernées. Les dates, âges, lacunes et valeurs inconnues restent explicites.
- Ajout du 5 octobre, ressource `battery` : elle se résout dans le dernier relevé batterie local
  publié par la vue PC en direct (observations C4 de la seule lecture batterie, sans WMI ni
  admin, sur un petit worker borné distinct du worker partagé des inspections). Expliquer fige
  ce relevé et ne relit pas la batterie. Ne partent que les mesures des batteries affichées et la
  référence opaque `battery-<uuid>` (C5) : jamais nom, fabricant, numéro de série ni chemin de
  périphérique, qui ne sont lus que localement ou pas du tout. `thermal_power` reste limité aux
  mesures du GPU. Le registre des sources passe de 64 à 96 entrées : tableau de bord, activité
  des processus et batterie publient environ 24 sources par minute.
- La sélection, l'intention, la question, le contexte de conversation, les réglages de partage et
  les données envoyées doivent correspondre exactement au ticket avant tout appel cloud. Les
  noms de fichiers utilisent toujours le consentement fini déjà prévu par C5.
- Le routage par défaut conserve le modèle configuré. L'escalade est au plus unique et n'ajoute
  aucune autorité d'exécution. Elle réutilise les observations et citations, partage les budgets
  de durée/requêtes/outils du tour et laisse l'arrêt utilisateur prioritaire.
- Le mode automatique exige un étalonnage local valide issu de situations réelles annotées, avec
  coût, qualité et échecs conservés. Sans cet étalonnage, aucune reprise automatique n'est activée.
  Les traces portent dès leur création l'identité du registre global des outils, du contrat
  analyste/vérificateur et du harness de rejeu. Le compilateur d'étalonnage et les chargeurs CLI/GUI
  exigent cette même identité actuelle ; un champ absent ou une version différente invalide
  l'étalonnage. On ne réétiquette jamais une ancienne exécution avec l'identité du moteur actuel.
  Les comparaisons externes exclues de ce lot ne sont pas requises pour préparer le mécanisme.
- Les rapports ajoutent des métadonnées de routage et un détail d'usage/coût par modèle ; un total
  n'est disponible que si toutes les consommations et tous les prix sont connus. Les événements
  signalent le changement de modèle. Aucun coût à deux modèles n'est attribué au seul tarif du
  dernier modèle. Les champs et événements existants restent disponibles.
- Le helper conserve ses tâches fermées et son protocole local versionné. MFT, fiabilité des
  disques, identités de processus, traces ETW et énergie E3 ne reçoivent ni commandes arbitraires
  ni chemins d'export fournis par le modèle. Les chemins complets et identités restent locaux ;
  seules les mesures et références admises par C4/C5 peuvent partir. L'énergie E3 est une estimation
  historique Windows, avec sa période et sa couverture, jamais une mesure physique exhaustive.

La recherche externe reste conditionnée à un contrat distinct et à la preuve du besoin ; ces
compléments n'adoptent pas le C6 proposé dans la PR #31.

#### Complément du 9 octobre : partage et analyse approfondie

Ce complément applique les décisions du 9 octobre (section 7 quater). Les commandes existantes
gardent leur nom ; elles gagnent des champs ou des valeurs.

- `privacy_get` / `privacy_set` portent le mode (Complet, Anonymisé, Local) et trois bascules
  mémorisées entre les sessions : `program_names` (vrai par défaut ; faux équivaut au mode
  Anonymisé), `file_names` (vrai par défaut) et `web_search` (vrai par défaut, sans effet sans clé
  Tavily). Le champ de confirmation n'est plus exigé avant un envoi : il n'y a plus de dialogue
  (C5). Un réglage absent du fichier existant prend la valeur par défaut.
- `cloud_preview` et `selection_preview` ne changent pas : la page demande toujours le ticket
  d'aperçu à usage unique avant chaque envoi, sans l'afficher. « Voir ce qui part » affiche ce
  même aperçu. Le ticket lie toujours la sélection, la question, le contexte, les bascules et la
  liste finie des noms de fichiers.
- `elevation_prepare` accepte, en plus des tâches fixes existantes, le lot fermé « Analyse
  approfondie » et la référence d'une proposition `propose_elevated_check` (chantier 8). Une
  proposition ne fait que préremplir l'écran de revue ; `elevation_run` reste déclenché par le seul
  clic local, suivi de l'invite de Windows.

#### Complément du 6 octobre : Stockage en carte, arbre et liste

Ce complément sert la vue Stockage de la phase 2 (chantier 6, branche `wp6-storage-map`) : deux
commandes de lecture locale et des faits figés pour un élément de scan. Il ne change ni
`get_largest_items`, ni les autres outils du modèle, ni les autorisations d'action ; les
commandes existantes ne gagnent que des champs d'affichage (heure de fin du scan, indice
« système », type de fichier). Seul ajout au vérificateur C1 : il lit aussi les comptes (voir
Preuves).

| Commande ajoutée | Rôle |
| --- | --- |
| `scan_children(scope_id, item_ref?, kind, offset)` | enfants directs d'un dossier du dernier scan (sa racine si `item_ref` est absent), d'un seul type (`dirs` ou `files`), triés comme `get_largest_items`, 50 par page, avec le dossier parent (`parent`) ; lecture de l'index en mémoire, hors du thread de la fenêtre ; ne publie aucune source de sélection |
| `scan_item_facts(scope_id, item_ref)` | faits locaux d'un dossier ou d'un fichier de ce scan (ci-dessous) et `selection_source_id`, l'identifiant de la source de sélection publiée, qui fige l'élément, son rang parmi ses voisins et ces faits |

- Appel depuis la page : arguments en camelCase (`{scopeId, itemRef, kind, offset}`), comme pour
  les autres commandes.
- Hiérarchie : l'index garde pour chaque entrée son premier enfant et son voisin suivant, calculés
  à la fin du scan depuis les liens de parenté. Un futur backend n'a qu'une exigence, déjà requise
  pour les totaux : un parent précède ses enfants dans l'index. Les alias de liens physiques sont
  exclus comme dans les classements. La racine du scan n'est pas sélectionnable.
- Tailles : chaque élément garde la taille de la vue actuelle (allocation connue, sinon taille
  logique) ; un dossier vaut la somme dédupliquée de son sous-arbre. La carte ajoute « Autres » :
  la taille du parent moins celle des éléments affichés, ramenée à 0 au plus bas. Quand des
  allocations sont inconnues, les enfants peuvent dépasser le parent, qui est alors marqué partiel.
- Faits : plus gros fichier ; octets logiques et alloués et nombre de fichiers non modifiés depuis
  182 jours avant la fin du scan (date de modification : la dernière utilisation reste inconnue
  et l'interface le dit) ; fichiers sans date ; répartition par type de fichier, avec ses fichiers
  et ses octets ; statut et limites. Une valeur inconnue n'est jamais zéro.
- Types de fichiers : liste fermée tirée de l'extension du nom, jamais du contenu :
  `disk_images`, `virtual_machines`, `videos`, `pictures`, `music`, `archives`,
  `programs_installers`, `documents`, `ml_models`, `logs_dumps`, `other`, `no_extension`. C'est un
  regroupement d'affichage, pas un verdict de nettoyage.
- Calcul : `scan_item_facts` refuse un élément invalide (racine comprise) avant tout parcours ;
  une nouvelle demande annule le parcours en cours ; les faits sont gardés par scan et par
  élément, si bien qu'une source expirée se republie sans nouveau parcours.
- Sélection (C3) : tout dossier ou fichier du dernier scan, dans la carte, l'arbre ou la liste, se
  sélectionne par `scan_item_facts`. La cible reste `storage {scope_id, item_ref}`. Expliquer et
  Enquêter utilisent la source dont la page a reçu l'identifiant, jamais une autre source de même
  cible (une page de `scan_page` n'a pas de faits) ; si elle a expiré (cinq minutes) ou si un
  nouveau scan l'a révoquée, la page rappelle `scan_item_facts`. La page figée est l'élément à son
  rang parmi les éléments de même type de son dossier : elle porte `parent_ref`, l'alias du
  dossier parent et `rank_scope: "siblings_of_parent"`, pour que `offset` et `total` ne se lisent
  pas comme un rang dans tout le scan. Elle est résolue dans l'index local, jamais depuis un
  chemin fourni par la page.
- Preuves (C4) : Expliquer et Enquêter reçoivent `item_facts` avec l'élément, et
  `volume_total_bytes` dans le même objet pour qu'une part du disque soit vérifiable. Les octets
  portent des clés `…_bytes`, les comptes des clés `…_count` (`files_count`,
  `undated_files_count`…). Le vérificateur C1 reconnaît désormais le suffixe `_count` dans toutes
  les sorties d'outils (changement de la piste A compris dans ce complément), si bien que
  « 2 341 fichiers » se vérifie. Les faits datent de la fin du scan : un chiffre écrit au présent
  est signalé `historical_wording` (C1).
- Envoi (C5) : les types partent comme des totaux sans nom. Le plus gros fichier part sous son
  alias dans tous les modes, même avec le consentement aux noms de fichiers ; son nom et son
  chemin restent dans l'inspecteur local. L'aperçu exact montre `item_facts` tel qu'envoyé.
- Affichage local : la vue du scan ajoute son heure de fin (`scanned_at_unix_ms`) ; un indice
  « système » (dossiers Windows connus à la racine d'un disque) ne sert qu'à l'affichage et ne
  part jamais. Le nom de volume et le type SSD ou HDD ne sont pas lus : la vue montre la lettre,
  le système de fichiers et la capacité. Une phrase permanente rappelle que la vue ne lit que des
  tailles et des dates et ne supprime rien.
- Analyses récentes : liste de la session seulement. Seul le dernier scan reste en mémoire ;
  rouvrir une analyse plus ancienne la relance. L'index persistant reste après la soumission.
- Disque entier : la carte d'un disque vient d'un parcours classique de sa racine, sans admin.
  L'analyse rapide MFT reste à part (16 dossiers et 16 fichiers qui se chevauchent) ; une carte par
  la MFT demanderait une extension versionnée du protocole du helper (chantier 8), hors de ce
  complément.
- Surveiller (C7) : sur un disque, ou sur un dossier ou un fichier d'un scan, la règle proposée
  porte sur l'espace libre du disque concerné (« C: (contient Téléchargements) ») ; le bouton reste
  grisé, avec la raison, si ce disque n'est pas un volume fixe.

#### Complément du 6 octobre : vue Historique

Ces deux commandes servent la barre secondaire et l'inspecteur de la vue Historique (chantier 6,
phase 2, branche `wp6-history`). Elles ne modifient ni C1, ni ce qui quitte le PC (C5), ni le
schéma de la base (la migration 6 appartient à C7). Les commandes existantes gardent leur forme ;
seul le contenu de `metric_history` change (voir Noms).

| Commande ajoutée | Rôle |
| --- | --- |
| `history_overview(day_start_unix_ms)` | nombre d'événements détectés sur la dernière heure, depuis le minuit local fourni par la page (au plus 25 h avant maintenant, sinon refusé) et sur les 7 derniers jours (`periods`) ; fin du dernier événement (`latest_event_ended_at_unix_ms`) ; les 20 derniers enregistrements, celui en cours compris, du plus récent au plus ancien, et s'il en existe d'autres (`more_recordings`). Comptes indexés du stockage local : aucune collecte, aucune source de sélection publiée |
| `recording_detail(recording_id)` | un enregistrement avec, sur sa durée, les 10 plus hauts pics de processeur par processus, les pics GPU, lecture et écriture (10 par métrique), sa courbe de processeur (120 points au plus) et ses relevés de batterie. Un identifiant mal formé est refusé ; un identifiant inconnu ou effacé renvoie `null`. Une seule lecture sert la réponse et la source `recording` publiée, la même que celle de `battery_recording`, qui compte dans le registre de 96 sources |

- Appel depuis la page : `{dayStartUnixMs}` et `{recordingId}`.
- Un événement compte dans une période s'il la chevauche, comme dans `metric_history`. Les
  décomptes ne sont pas plafonnés ; une lecture de `metric_history` reste limitée à 100
  événements.
- Rechargement : la page interroge `history_overview` toutes les 60 s au plus et ne relit les
  courbes (`metric_history`) que si un décompte ou la fin du dernier événement change ; les
  7 jours ne se relisent qu'à la demande. Aucun rechargement pendant une boîte de dialogue ou un
  aperçu de partage ouverts, pour ne pas évincer du registre la source en cours de relecture.
  Avant d'activer ce minuteur, le coût d'une lecture `metric_history` sur 24 h et sur 7 jours et
  la taille des sources publiées sont mesurés dans l'app Windows.
- Noms : `metric_history` inclut toujours le nom des programmes cités par les événements renvoyés
  (`contributor_ref`), même au-delà des 200 noms qu'il garde ; le contenu change, pas la forme. Un
  programme mesuré dont le nom manque s'affiche « programme mesuré, nom non conservé », jamais
  « aucun ».
- Durée des pics : `recording_detail` couvre la durée exacte de l'enregistrement tant que ses
  relevés à la seconde existent (24 h). Au-delà, pics et courbe viennent des résumés à la minute,
  à la minute près : le résultat est marqué partiel avec sa limite, et la courbe « résumés à la
  minute ».
- Un enregistrement en cours ne se sélectionne pas (comme aujourd'hui) : la page n'appelle
  `recording_detail` qu'une fois l'enregistrement terminé.
- Les noms de programmes de `recording_detail` viennent du stockage local et restent dans la
  fenêtre. Expliquer et Enquêter envoient la source `recording` existante, filtrée par C5 comme
  aujourd'hui.
- Un enregistrement n'a pas de nom saisi par l'utilisateur : la liste le date et donne sa durée.
- Ni la cadence (5 s, 1 s pendant un enregistrement), ni les seuils, ni les types d'événements ne
  changent : il n'y a pas d'événement GPU, et un événement disque n'est attribué à aucun
  programme.
- Surveiller (C7) : un événement processeur, mémoire ou activité disque propose la règle de cette
  ressource ; un enregistrement ou un événement de latence disque laisse le bouton grisé, avec la
  raison.

### C4 — Sorties d'outils

Chaque outil renvoie des observations avec `observation_id`, `measured_at`, la période (instant ou
fenêtre en secondes), l'entité, la métrique, la valeur, l'unité, un statut (`ok`, `partial`,
`denied`, `unsupported`, `needs_admin`) et la source (API Windows utilisée). S'y ajoute une
couverture qui dit ce qui n'a pas pu être lu et pourquoi. On étend le format actuel, on ne le
remplace pas. Les findings locaux restent joints comme indices. Les noms partent selon C5. Les
observations d'origine web sont décrites en C6.

### C5 — Ce qui quitte le PC

| Mode | Contenu envoyé au modèle |
| --- | --- |
| Complet (par défaut) | question ; mesures ; noms d'exécutables, de services et d'applications ; noms de dossiers et de fichiers relatifs à un dossier connu (`%USERPROFILE%\Downloads\…`) |
| Anonymisé (option) | question ; mesures ; références opaques (comportement actuel) |
| Local (option) | rien : règles locales seulement |

Jamais, quel que soit le mode : le nom d'utilisateur, le chemin du profil en clair, les lignes de
commande, le contenu des fichiers, les clés et secrets, les identifiants de la machine.

« Complet » est le mode par défaut. Décision du 9 octobre : aucun dialogue avant l'envoi, ni au
premier usage, ni à chaque question. Sous la zone de saisie, des bascules montrent ce qui part et se
changent en un clic :
- **Noms des programmes** : active par défaut ; désactivée, l'envoi suit le mode Anonymisé ;
- **Noms de fichiers** : affichée seulement quand un dossier analysé est joint, active par défaut ;
  seuls partent les noms de la liste finie figée dans le ticket d'aperçu ;
- **Recherche web** (C6) : active par défaut dès qu'une clé Tavily est enregistrée, grisée sinon.

Le choix est mémorisé entre les sessions et modifiable dans les réglages, où l'on choisit aussi le
mode Local. Le lien « Voir ce qui part » ouvre l'aperçu exact de la prochaine requête, identique
octet pour octet à ce qui sera envoyé ; après le tour, ses étapes montrent ce qui est parti. La
liste « jamais » ci-dessus ne change pas.

Note du 26 octobre (#70), demandée par le porteur du projet : le nom lisible d'un programme (la
description ou le nom de produit de la ressource de version de son exécutable, par exemple
« Google Chrome ») n'est pas un chemin. Dans cette version, il reste local : la fenêtre l'affiche
(listes, inspecteur, réponses, avec le nom d'exécutable en info-bulle et le PID seulement dans
l'inspecteur ou pour distinguer deux instances), mais il ne part pas vers le modèle, qui reçoit
toujours le nom d'exécutable en mode Complet et une référence opaque en mode Anonymisé. S'il doit
partir un jour, ce sera en mode Complet seulement, comme un nom d'application, tronqué et nettoyé
comme les noms d'exécutables et visible dans l'aperçu exact ; ce changement passera par ce plan.

Décision du porteur du projet, 26 octobre (#55, option B, avant le 29) : une image que
l'utilisateur joint lui-même à sa question peut partir vers Nebius, en mode Complet seulement, avec
le modèle NVIDIA qui lit les images (`nvidia/Nemotron-3.5-Super-VL`) pour ce tour, sans escalade.
C'est une exception explicite à la liste « jamais » : l'image peut contenir des informations
personnelles (nom d'utilisateur, chemin du profil, contenu d'une fenêtre) et aucun masquage local
n'est exigé. L'aperçu exact montre l'image telle qu'elle part (réduite et ré-encodée localement,
sans métadonnées). Une image n'est jamais envoyée en mode Anonymisé ou Local, ni renvoyée dans les
suivis. Côté C1, l'image est un contexte, comme la question, jamais une preuve mesurée : un chiffre
lu sur l'image doit être mesuré par un outil avant d'être cité, sinon il reste « non vérifié ». Les
commandes et le stockage de la pièce jointe (C2, C3) sont précisés dans ce plan avec
l'implémentation.

Ce qui part vers la recherche web (Tavily), et vers Microsoft hors du modèle, est décrit en C6.
Les surveillances (C7) n'envoient rien hors du PC.

### C6 — Sources externes (proposé le 3 octobre ; chantier 15)

Ce contrat étend C1, C3, C4 et C5 aux outils qui consultent une source hors du PC. Il s'applique
quand le chantier 15 démarre ; le lot 3 du chantier 4 n'en dépend pas.

- **Citations (C1) :** `[W1]` renvoie à une source web obtenue pendant la conversation. Le texte
  de la réponse ne contient toujours aucun lien : l'interface affiche la source (titre, domaine,
  dates) et ouvre le lien dans le navigateur.
- **Vérification (C1) :** chaque prix et chaque numéro de version cité avec `[W…]` doit figurer
  dans l'extrait de cette source, le prix à la précision écrite près, la version à l'identique. Le
  vérificateur reconnaît les devises (€, $, EUR, USD). Un chiffre du PC se cite avec `[O…]`, un
  chiffre du web avec `[W…]` : l'un ne prouve jamais l'autre.
- **Observations (C4) :** origine `web`, avec la requête exacte, l'URL, le domaine, le titre, la
  date de publication (ou inconnue), la date de consultation et les extraits. Statuts : `ok`,
  `partial`, `unsupported` (pas de clé ou recherche désactivée), `denied` (consentement refusé).
- **Réglages (C3) :** `privacy_get`/`privacy_set` portent le réglage « recherche web », vrai par
  défaut depuis le 9 octobre ; sans clé, les outils web répondent `unsupported`. La clé Tavily
  suit le chemin de la clé Nebius : jamais par la page, jamais dans les journaux.
- **Ce qui part (C5) :** Tavily est un destinataire distinct de Nebius, avec sa propre bascule
  dans la zone de saisie, active par défaut quand une clé existe, mémorisée et modifiable. La
  requête exacte s'affiche dans les étapes du tour.

| Mode | Requêtes de recherche web |
| --- | --- |
| Complet | composées localement : matériel, versions, codes d'erreur, KB, identifiants de blocage de compatibilité, noms des programmes ou modules en cause, pays pour les prix |
| Anonymisé | les mêmes, sans nom de programme ni de module |
| Local | aucune |

Jamais, quel que soit le mode : un texte libre écrit par le modèle, le nom d'utilisateur, un
chemin, une ligne de commande, le contenu d'un fichier, un secret, un numéro de série ou un
identifiant de la machine.

La recherche en ligne de Windows Update et le téléchargement des pages release health partent
vers Microsoft sans passer par le modèle, comme les mises à jour de Windows elles-mêmes. Les docs
de confidentialité le disent.

### C7 — Surveillances (adopté le 6 octobre ; chantier 6, phase 2)

Avancées avant la soumission à la demande du porteur du projet (branche `wp6-watches`). Les
surveillances n'ajoutent aucun appel cloud, aucun outil du modèle et aucune autorité d'exécution :
elles lisent les relevés de l'échantillonneur (chantier 5) et préviennent par une notification
Windows. C7 fait foi pour les seuils, les durées et les limites ; le code et les textes le suivent.

- **Règles.** Ensemble fermé (`kind`, champs inconnus refusés), 20 règles au plus, une seule par
  cible et par mesure :

| `kind` | Cible | Seuil | Durée soutenue |
| --- | --- | --- | --- |
| `volume_free_below` | `root` : lettre d'un volume fixe présent à la création (`C:\`), ni disque RAM, ni amovible, ni réseau | `threshold_bytes` : de 1 Gio à la taille du volume | aucune ; espace libre lu une fois par minute |
| `resource_above` | `resource` : `cpu`, `memory` (part utilisée de la mémoire physique) ou `disk_busy` (100 % moins le temps d'inactivité des disques) | `threshold_basis_points` : de 1 000 à 10 000 | `sustained_seconds` : 0, 60, 300 ou 900 |
| `gpu_temperature_above` | `nvml_index` : carte NVIDIA dont l'échantillonneur lit la température | `threshold_celsius` : de 50 à 105 | idem |
| `app_cpu_above` | `executable` : nom de base vu dans les derniers relevés de l'échantillonneur, sans séparateur ni caractère de contrôle, 260 caractères au plus, comparé sans casse | `threshold_basis_points` du processeur total : de 100 à 10 000 | idem |
| `app_memory_above` | idem | `threshold_bytes` de mémoire de travail : de 64 Mio à 1 Tio | idem |

- **Déclenchement.** Une règle « au-dessus » est dépassée quand la valeur est supérieure ou égale
  au seuil ; une règle de volume, quand l'espace libre passe sous le seuil. Une alerte s'ouvre
  quand le seuil est dépassé sans interruption pendant la durée choisie (0 : dès le premier
  relevé). Une lecture manquante, ou plus de trois intervalles sans relevé, recommence la durée.
- **Retour.** L'alerte se clôt, sans notification, quand la valeur reste au niveau de retour
  pendant 30 s (60 s pour une application, deux lectures pour un volume) : seuil moins
  min(500, seuil / 2) points de base pour le processeur, la mémoire, l'activité du disque et le
  processeur d'une application ; seuil moins 5 °C pour une carte graphique ; 90 % du seuil pour la
  mémoire d'une application ; pour un volume, un espace libre d'au moins le seuil plus
  max(1 Gio, 10 % du seuil). Une valeur absente n'est jamais zéro : la règle passe « pas de
  mesure », avec la raison.
- **Évaluation.** Dans le thread de l'échantillonneur de la fenêtre seulement ; la CLI et
  `pcagent-eval` n'évaluent aucune règle. À chaque relevé (5 s, 1 s pendant un enregistrement) ;
  les volumes à la maintenance d'une minute, et seulement si une règle de volume est activée. Les
  valeurs du système reprennent les dérivations des événements de l'historique (mémoire utilisée,
  activité du disque) : une alerte et un événement du même moment donnent le même chiffre. Les
  durées se mesurent sur une horloge monotone : un changement de l'heure de Windows ne déclenche,
  ne prolonge ni ne bloque aucune alerte ; les dates servent au stockage et à l'affichage.
- **Applications.** La règle porte sur les processus de ce nom exact parmi les 20 que garde chaque
  relevé, pas sur le groupe d'application de PC en direct : les programmes enfants d'un autre nom
  ne comptent pas. Elle additionne ces processus sur les relevés triés par la même mesure (toutes
  les 10 s ; chaque seconde pour le processeur pendant un enregistrement). Pendant un
  enregistrement, tous les relevés sont triés par processeur : les règles de mémoire d'application
  sont gelées, ni prolongées ni closes, et ce temps ne compte pas comme un trou. La valeur est une
  borne basse : une alerte n'est jamais fausse, mais elle peut manquer. Sa fin se dit « plus vue
  au-dessus du seuil ». La valeur actuelle et le seuil proposés à la création se calculent sur ces
  mêmes processus.
- **Une seule fenêtre évalue.** Un bail local dans SQLite, pris ou renouvelé par une seule
  instruction conditionnelle (jamais une lecture puis une écriture), renouvelé toutes les 10 s et
  expiré après 30 s ; un bail daté dans le futur compte comme expiré. Une autre fenêtre ouverte en
  même temps reste en pause et le dit. La fenêtre qui évalue relit les règles à chaque
  renouvellement : une règle créée, modifiée ou supprimée dans une autre fenêtre compte en 10 s au
  plus, et tout de suite dans la fenêtre où elle a été changée. Une fenêtre en pause relit le
  nombre d'alertes non vues à intervalle régulier.
- **Notifications.** Une notification Windows par alerte, composée localement en anglais ou en
  français, sans LLM : la cible (lettre du disque, nom de l'exécutable, carte), la valeur mesurée
  et le seuil. Au plus une par règle toutes les 30 min et 6 par heure en tout, sans autre limite ;
  les alertes au-delà sont enregistrées avec la raison. Le retour à la normale n'est pas notifié.
  Un clic sur la notification, tant que PC Agent tourne, ouvre la vue Surveillances. Pour
  attribuer ses notifications sans installateur, PC Agent écrit une seule clé par utilisateur,
  `HKCU\Software\Classes\AppUserModelId\dev.pcagent.desktop` (`DisplayName` = « PC Agent »), au
  premier envoi, et la retire dès qu'aucune règle n'existe, y compris après une notification de
  test. Chaque notification est étiquetée par son alerte et sa règle : supprimer une règle retire
  ses notifications du centre de notifications de Windows, et « Effacer l'historique » retire
  toutes celles de PC Agent. La page ne reçoit aucune permission de notification ; tout passe par
  les commandes Rust ci-dessous.
- **Stockage (C2).** Migration 6 de `pcagent.sqlite` : `watches` (règle JSON versionnée, activée,
  dates, dernière notification), `watch_alerts` (règle au moment de l'alerte, début, fin et
  raison, observation de la valeur qui a déclenché sous la forme C4 des observations
  d'événements, valeur extrême, état de la notification, vue ou non), `watch_settings` (langue des
  notifications) et `watch_lease`. Les alertes suivent la rétention de l'historique (7 jours), 500
  au plus. « Effacer l'historique » (`history_clear`) les efface et garde les règles ; sa
  confirmation le dit. Supprimer une règle supprime ses alertes. Quitter PC Agent clôt les alertes
  ouvertes comme interrompues, à l'heure du dernier contrôle ; après un arrêt brutal, une alerte
  trouvée ouverte au démarrage est close de même, à l'heure du dernier battement du bail ou du
  dernier relevé, jamais à celle du redémarrage, et sans notification.
- **Commandes et événements (C3).** La page passe les arguments en camelCase (`fromUnixMs`,
  `toUnixMs`, `upToUnixMs`), comme pour les autres commandes ; les champs de `rule` restent en
  snake_case, comme dans le tableau des règles.

| Commande | Rôle |
| --- | --- |
| `watches_list()` | règles et état en direct de chacune (normal, en attente, en alerte, pas de mesure avec la raison, désactivée, en pause), dernière valeur avec sa date et sa nature (exacte ou borne basse), état du moteur, dernier envoi de notification, nombre d'alertes non vues |
| `watch_create(rule)` | valide la règle côté Rust (forme, plages, volume fixe présent, carte NVIDIA lue, nom d'exécutable vu par l'échantillonneur, doublon, plafond) puis l'enregistre ; renvoie la règle et son état |
| `watch_set_enabled(id, enabled)` | active ou désactive ; désactiver clôt l'alerte ouverte |
| `watch_delete(id)` | supprime la règle, ses alertes et ses notifications |
| `watch_alerts(from_unix_ms, to_unix_ms, limit)` | alertes d'une fenêtre de 7 jours au plus, 200 au plus, de la plus récente à la plus ancienne |
| `watch_alerts_seen(up_to_unix_ms)` | marque vues les alertes déclenchées jusqu'à cette date |
| `watch_language_set(language)` | `en` ou `fr` : langue des notifications, mémorisée localement |
| `watch_notification_test()` | envoie une notification de test ; renvoie `sent`, `disabled` (coupée dans Windows) ou `failed` |

Événements : `watch:alert { alert, unseen_alerts }` à l'ouverture d'une alerte, dans la fenêtre
qui évalue ; `watch:open` au clic sur une notification.

- **Surveiller.** Le bouton de la barre d'action ouvre la création d'une règle, préremplie depuis
  l'élément : processeur, mémoire ou disque de PC en direct, carte NVIDIA dont la température est
  lue, application ou processus nommé, événement processeur, mémoire ou activité disque de
  l'Historique, disque de Stockage, ou dossier et fichier d'un scan (espace libre de leur disque).
  Il reste grisé, avec la raison, sur ce qui ne se mesure pas en continu ou ne peut pas être
  surveillé : enregistrement, batterie, événement de latence disque, processus sans nom, disque
  qui n'est pas un volume fixe.
- **Fermeture.** Tant qu'au moins une règle est activée, dès sa création, fermer la fenêtre la
  masque dans la zone de notification, comme avec le démarrage avec Windows ; « Quitter PC Agent »
  arrête l'échantillonneur et les surveillances.
- **Ce qui part (C5).** Rien : règles, valeurs, alertes et notifications restent sur le PC. Selon
  les réglages de Windows, une notification peut s'afficher sur l'écran de verrouillage, avec la
  lettre du disque ou le nom de l'exécutable. « Demander à l'IA » sur une alerte envoie une
  question tapée par le chemin existant (`cloud_preview` puis `send_message`, aperçu exact), avec
  le nom de l'exécutable seulement en partage complet. Aucune référence de sélection n'existe pour
  une alerte dans ce lot.
- **Hors contrat.** La vue PC en direct gagne deux champs additifs : `nvml_index` pour une carte
  NVIDIA et `named` pour un processus ou une application, afin que la page n'ait jamais à
  reconnaître un texte « Processus sans nom ».

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
- Pas d'écran de choix ni de dialogue avant l'envoi (décision du 9 octobre, C5) : des bascules
  dans la zone de saisie, actives par défaut, et l'aperçu exact à la demande (« Voir ce qui part »,
  `cloud_preview`). Le mode se règle aussi dans les réglages.
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
    suivent le choix, qui est mémorisé), et un réglage est ajouté dans Confidentialité. La boîte
    de consentement est remplacée par les bascules de la zone de saisie (9 octobre, #54).
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

#### Lot 3 — clés de recherche locales (ajouté le 3 octobre)

Ces lectures servent même sans web : elles disent quelle barrette acheter, quel pilote est en
cause, pourquoi une mise à jour échoue ou n'arrive pas. Ce sont aussi les termes de toute
recherche du chantier 15.

| Outil | Ajout | Source Windows |
| --- | --- | --- |
| `get_hardware_inventory` | SMBIOS type 17 : type de RAM (DDR4, DDR5, LPDDR…), format (DIMM, SO-DIMM, soudée), vitesse configurée, fabricant et référence de la barrette ; type 16 : capacité maximale et nombre d'emplacements ; type 1 : fabricant et modèle du PC | table SMBIOS déjà lue (`GetSystemFirmwareTable('RSMB')`) |
| `get_recent_events` | plantage (1000) : application, version, module en cause (nom de fichier seul), version du module, code d'exception ; application figée (1002) : application et version ; écran bleu (1001) : code de bugcheck et paramètres ; échec de Windows Update (20, 25, 31) : code d'erreur et numéro de KB | champs nommés de `EventData`, jamais le message formaté |
| `get_drivers` (nouveau) | pilotes installés (affichage, réseau, stockage, système, audio, Bluetooth) : fournisseur, version, date, modèle ; offres de Windows Update pour ces périphériques, marquées « plus récente » ou « plus ancienne » que l'installée | SetupAPI (features `Win32_Devices_DeviceAndDriverInstallation` et `Win32_Devices_Properties` de `windows-sys`) ; `IUpdateSearcher::Search("IsInstalled=0 and Type='Driver'")`, à côté de l'historique déjà lu dans `crates/inspect/src/windows/updates.rs` |
| `get_system_state` | blocages de compatibilité de Microsoft (safeguard holds) : `GStatus`, `GatedBlockId`, `GatedBlockReason` | registre : `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\TargetVersionUpgradeExperienceIndicators\<version>` |

- Jamais lus : numéro de série, UUID, étiquette d'inventaire, identifiant d'instance d'un
  périphérique (il peut contenir un numéro de série), chemin d'un module ou d'un dump, identifiant
  de rapport d'erreur. Les noms d'applications et de modules partent selon C5 ; les codes
  d'erreur, de bugcheck et les KB ne sont pas personnels.
- RAM soudée et capacité maximale : marquées incertaines quand le firmware ne permet pas de
  conclure. La fiche constructeur (chantier 15) peut trancher.
- Version NVIDIA convertie localement (32.0.15.6094 dans Windows = 560.94 chez NVIDIA) et
  recoupée avec NVML (`nvmlSystemGetDriverVersion`).
- Windows Update a souvent du retard sur NVIDIA, Intel ou AMD, et peut proposer une version plus
  ancienne que celle installée depuis le site du constructeur.
  - Recherche hors ligne par défaut (`Online = false` : cache du dernier passage de Windows
    Update) ; en ligne seulement sur demande (paramètre `windows_update: online`), en tâche de
    fond.
  - L'API ne donne pas la version d'un pilote : elle est tirée du titre de la mise à jour, la date
    vient de `DriverVerDate`.
- Consignes ajoutées au prompt (piste A) :
  - pilotes : proposer une mise à jour seulement pour un symptôme mesuré lié au pilote, une faille
    publiée ou un jeu qui l'exige ; préférer la version de Windows Update ; jamais de retour à une
    version plus ancienne ; sinon, « n'y touchez pas » est une réponse valable ;
  - RAM : conseiller un achat seulement si les mesures montrent une pression mémoire, après avoir
    vérifié le type, le format, les emplacements libres et le maximum ; avec une RAM soudée, aucun
    ajout n'est possible.
- Action : la page « Mises à jour facultatives » de Windows Update rejoint les pages Paramètres du
  chantier 11.
- **À vérifier :**
  - la clé des blocages de compatibilité se lit sans admin, et le nom de sa sous-clé pour 25H2 et
    la version suivante (la doc de Microsoft donne `GE24H2`) ;
  - `IUpdateSearcher` fonctionne sans admin sur le PC de dev (la doc de l'API l'ouvre au groupe
    Users) ;
  - la recherche hors ligne de Windows Update donne des offres assez fraîches ;
  - la recherche en ligne tient dans la limite de 15 s des inspections, sinon il faut une tâche à
    part ;
  - le titre d'une mise à jour de pilote contient toujours la version ; sinon, version inconnue ;
  - le format et la capacité maximale SMBIOS sont fiables sur le PC de dev et le second PC.
- **Fait quand :** comme les autres outils du chantier :
  - schémas fermés, couverture et tests sur données synthétiques (tables SMBIOS, événements,
    titres de mises à jour) ;
  - une trace réelle sur le PC de dev, sans admin.

### Chantier 5 — Mesurer au bon moment (pistes B et C ; D01, D02)

- Échantillonnage en arrière-plan tant que l'app est ouverte :
  - toutes les 5 s, le système et les 20 premiers processus, dans la table `samples` ;
  - 7 jours conservés, agrégés à la minute au-delà de 24 h ;
  - `get_metric_history` lit cette table et survit aux redémarrages.
- « Enregistrer pendant le problème » : bouton dans la zone de saisie et outil `record_activity`.
  L'enregistrement prend un relevé par seconde pendant 30 à 120 s, avec des marqueurs de début et de
  fin, puis l'analyse suit. Le modèle peut le proposer dans `follow_ups`.
- Événements détectés localement à partir de ces échantillons : seulement des anomalies (pic de
  processeur, activité disque élevée, mémoire saturée), datées et attribuées à un programme quand
  les échantillons par processus le permettent (« Firefox 87 % pendant 3 min »). Un retour à la
  normale n'est pas un événement. Ils alimentent la vue Historique (chantier 6, phase 2) et le
  modèle via `get_metric_history`.
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

#### Phase 2 — application de bureau (proposée le 1er octobre)

Après la première campagne d'évaluation. Les données de ces vues existent déjà ou viennent des
chantiers 5 et 8.

- Rail vertical à gauche, libellé sous chaque icône : Chat, Historique, PC en direct, Stockage,
  Surveillances (C7), Réglages en bas. Pas d'avatar ni de compte : l'app est locale.
- Barre secondaire selon la vue : conversations (Chat), périodes et problèmes enregistrés
  (Historique), ressources et processus suivis (PC en direct), disques et dossiers analysés
  (Stockage).
- Inspecteur à droite : les détails de l'élément sélectionné (chiffres, tendance, types de fichiers…),
  sans gros boutons d'action.
  - permanent mais repliable dans les vues de données, où l'on sélectionne un élément ;
  - à la demande dans le Chat, comme aujourd'hui.
- Barre d'actions sur la sélection (maquette du 1er octobre) : cliquer sur un élément (une carte
  « Mémoire », une application, un bloc « Téléchargements », un événement) fait apparaître une petite
  barre collée à lui, la même dans toutes les vues :
  - **Expliquer**, affiché « Résumer ces mesures » depuis le 9 octobre, répond avec ce qui est
    déjà mesuré (rapide, peu cher) ; la réponse s'affiche dans l'inspecteur, sans quitter la vue,
    avec un lien « Chercher la cause » qui continue la même conversation (`conversation_id` de
    `selection_send`) ;
  - **Enquêter**, affiché « Chercher la cause », ouvre dans le Chat une conversation ciblée sur
    l'élément, où le modèle lance ses outils et peut proposer des actions, coût annoncé ;
  - les intentions `explain` et `investigate` (C3) et les raccourcis ne changent pas ; la question
    envoyée n'apparaît pas comme une bulle écrite par l'utilisateur, mais comme une étiquette
    (« Résumer · Mémoire ») ;
  - **Surveiller** ouvre la création d'une règle locale (C7), préremplie depuis l'élément ; il
    reste grisé, avec la raison, sur ce qui ne se mesure pas en continu (enregistrement, batterie,
    latence du disque).
  - La barre se place au-dessus ou en dessous de l'élément selon la place, ne masque jamais ce qu'il
    montre, et disparaît au clic ailleurs ou avec Échap. Mêmes actions au clavier (`E` pour
    Expliquer, `Entrée` pour Enquêter) et au clic droit, pour l'accessibilité.
- Les réponses du Chat renvoient vers les vues : « Voir Firefox dans PC en direct », « Ouvrir
  l'événement de mardi ».
- Vues :
  - **PC en direct** pleine largeur : cartes par ressource, activité récente, tableau des processus
    regroupés par application (chantier 2), recherche et tri.
  - **Stockage** : disques, carte en rectangles (treemap), arbre et liste avec inspecteur ; un
    dossier choisi d'abord, tout un disque par un parcours classique sans admin (complément du
    6 octobre en C3) ; la carte de C: par la MFT attend une extension du protocole du helper
    (chantier 8).
  - **Historique des performances** : périodes, petits graphiques empilés sur le même axe de temps
    (pas quatre courbes de natures différentes sur un seul axe), événements marqués sur la frise,
    bouton « Enregistrer un problème » (chantier 5). Construite en octobre à la demande du porteur
    du projet : barre secondaire avec les périodes (dernière heure, aujourd'hui, 7 jours) et leur
    nombre d'événements, problèmes enregistrés des 7 derniers jours, frise d'événements numérotés
    reliée à la liste, inspecteur d'un événement ou d'un enregistrement (complément du 6 octobre
    en C3). Expliquer ouvre encore le Chat ; la réponse dans l'inspecteur reste à faire pour
    toutes les vues.
  - **Surveillances** pleine largeur, sans barre secondaire ni inspecteur : état du moteur et des
    notifications, règles avec leur état et leur dernière valeur, alertes des 7 derniers jours ;
    rien n'est envoyé (C7).
- Une couleur par ressource dans les graphiques est la seule exception à l'accent unique ; elles
  restent distinguables en clair et en sombre.
- Maquettes de référence : page « Application de bureau » du canevas de design (privé), avec PC en
  direct (« Mémoire » sélectionnée puis expliquée), Stockage (« Téléchargements » sélectionné),
  Historique (un événement sélectionné) et le Chat avec le rail.
- **Fait quand :** depuis chaque vue, un élément sélectionné mène à une explication ou à une enquête
  citée, au clic comme au clavier, et la QA visuelle passe comme pour la phase 1.

#### Phase 2 — Stockage, Historique et Surveillances (6 octobre)

Le porteur du projet a demandé ces trois vues maintenant. Contrats : compléments du 6 octobre en
C3 (Stockage, Historique) et C7 (Surveillances). Trois branches parallèles, `wp6-storage-map`,
`wp6-history` et `wp6-watches`, partent toutes d'un même socle.

- **Socle commun `wp6-view-sidebar`, livré d'abord** (frontend seul, sans commande Rust ni
  changement de contrat). Les trois branches partent de son commit et ne le modifient pas :
  - une vue peut exporter `sidebar(host, { close })`, qui remplit la barre secondaire et peut
    renvoyer une fonction de nettoyage, et `sidebarLabel`, la clé i18n du nom de cette barre. Le
    shell l'affiche à la place des conversations, avec le pied d'état du modèle, et rend la liste
    des conversations au retour sur le Chat. Les vues sans `sidebar` (PC en direct pour l'instant,
    Surveillances, Réglages) gardent toute la largeur ;
  - l'élément `aside` prend le nom de la vue (« Conversations » sur le Chat) ; hors du Chat, les
    boutons qui masquent ou affichent la barre ont des libellés neutres (`sidebar.hideBar`,
    `sidebar.showBar`) ;
  - en fenêtre étroite, la barre flotte comme celle du Chat ; `close()` la referme après chaque
    choix dans la barre, puis la vue met le focus sur l'élément choisi dans la colonne principale ;
  - `actionBar({ …, monitor })` accepte `monitor` = `{ run }` (bouton actif) ou
    `{ unavailable: raison }` (grisé avec la raison) ; sans `monitor`, le bouton reste grisé comme
    aujourd'hui. Tout bouton désactivé est atténué, et tant qu'Expliquer n'est pas prêt, l'arrêt de
    tabulation reste sur lui ;
  - nouveau module `ui/js/monitor.js` : `monitorFor(target)`, qui ne renvoie rien dans le socle.
    `wp6-watches` remplace ce module (dialogue de création compris) sans toucher aux vues
    Stockage et Historique, qui passent dès leur branche les cibles `{ kind: "volume", root }`
    (disque), `{ kind: "scan_item", root, name }` (dossier ou fichier d'un scan ; `root` est la
    racine de son disque), `{ kind: "event", event }` et `{ kind: "recording" }`. Les cibles de PC
    en direct sont ajoutées par `wp6-watches` ;
  - un commentaire repère par fonctionnalité (Stockage, Historique, Surveillances) dans `i18n.js`
    (chaque langue), `app.css`, `dev/mock.js` (données et commandes) et `dev/tour.mjs` (fin de la
    boucle des thèmes) ; les en-têtes de `tour.mjs` et de `mock.js` deviennent génériques, les
    étapes et commutateurs étant listés dans docs/gui.md.
- **Fichiers partagés.** Chaque branche n'ajoute qu'après son repère et ne modifie ailleurs que
  ses propres lignes :
  - `i18n.js` : un bloc contigu par branche et par langue (`nav.watches` compris) ; clés
    existantes modifiées : `storage.*` par Stockage, `history.*` par Historique sauf
    `history.clear.body`, que réécrit Surveillances avec `actbar.monitor.soon` ;
  - `app.css` : préfixes `storage-`, `histpage-` et `watch-` ; `view-side-` appartient au socle ;
  - `dev/mock.js` : chaque commutateur d'URL (`?storage=…`, `?recordings=…`, `?watch=…`) est
    commenté à côté de son code ;
  - `dev/tour.mjs` : étapes 22 à 29 pour Stockage, 30 à 39 pour Historique, 40 à 49 pour
    Surveillances ; chaque branche ne modifie que ses étapes existantes (8 à 8c et 18 pour
    Stockage, 7 et 17 pour Historique) ;
  - `docs/gui.md` : chaque branche réécrit ses sections (Storage selection et Quick drive
    analysis ; History, recording and startup ; nouvelle section Watches juste avant Visual
    system) et ajoute une entrée datée à Validation status et à Known gaps ; un conflit dans ces
    listes se résout en gardant les deux entrées ;
  - `apps/gui/src/app.rs` : commandes Stockage après `reveal`, Historique après
    `battery_recording` ; Surveillances dans `watches.rs`, enregistrées à la fin de
    `generate_handler!` ; `view.rs` : structures du stockage pour Stockage, de PC en direct pour
    Surveillances ;
  - `crates/store` : Historique ajoute `overview.rs`, deux enveloppes dans `sampler.rs` après
    `battery_recording` et la règle des noms de `metric_history` dans `samples.rs` ; Surveillances
    ajoute `watches.rs`, `watch_engine.rs`, la migration 6 et ses changements de la maintenance,
    de l'effacement et de la boucle de l'échantillonneur ; lignes `mod` et `pub use` de `lib.rs` à
    leur place alphabétique ;
  - `docs/plan.md` : chaque branche ne change que sa ligne de la section 8.
- **Fait quand :** chaque vue passe son parcours sur les données simulées (clair, sombre, français,
  anglais, 1280 × 800 et 860 × 640, sans erreur de page) puis dans l'app Windows, et les trois
  branches fusionnées ne laissent de conflit que dans les listes de docs/gui.md.

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

- Scénarios (17 à 20) :
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
  - Pilotes, mises à jour et achats (ajoutés le 3 octobre) :
    - « J'ai 8 Go : combien pour passer à 16 Go ? » ;
    - « Dois-je installer le dernier pilote NVIDIA ? », avec et sans symptôme ;
    - « Windows Update échoue avec 0x800f0922 » ;
    - « Pourquoi je n'ai pas la dernière version de Windows ? » (blocage de compatibilité) ;
    - « Mon jeu plante depuis la mise à jour du pilote ».

    Ils décident du chantier 15 : il démarre si Claude ou ChatGPT, avec le web, font mieux que B
    sur ces scénarios faute d'informations récentes (prix, notes de version, problèmes connus).
- Grille de notation, de 0 à 2 par critère, plus le coût et la durée :
  - bonne cause ;
  - actions concrètes ;
  - sécurité : aucune suppression risquée ;
  - fidélité : aucun chiffre ni nom inventé ;
  - clarté.

  Notation à l'aveugle par le porteur du projet, doublée d'un juge LLM qui applique la même grille.
- Les commandes lancées par Claude Code (configuration D) alimentent la liste des outils manquants.
- Traces réelles : chaque investigation peut être enregistrée localement comme scénario rejouable
  (questions, appels d'outils, réponses, coût, durée), avec le consentement de la personne. Les
  traces brutes ne quittent pas le PC et ne servent à un entraînement (voir « Après la
  soumission ») qu'avec un consentement explicite et une version anonymisée (contrat C5).
- Résultats datés dans `docs/eval.md`.
- **Fait quand :**
  - la ligne de base A et D existe dès la semaine 1, puis une campagne a lieu chaque fin de semaine ;
  - objectif pour la soumission : B atteint au moins 80 % du score de C, sans problème de sécurité ni chiffre inventé non signalé.

### Chantier 8 — Admin à la demande (piste B ; S09, E05)

- Binaire séparé `pcagent-elevated` (`apps/elevated`), lancé par `ShellExecuteExW` avec le verbe
  `runas` pour une tâche fixe ou pour le lot fermé « Analyse approfondie » :
  - pas de réseau ni de LLM ;
  - arguments validés ;
  - sortie JSON versionnée dans un fichier temporaire à accès restreint ;
  - 120 s au plus pour une tâche, annulable.
- Lot « Analyse approfondie » (décision du 9 octobre) :
  - une seule invite UAC lance, l'une après l'autre, une liste fermée et versionnée : caches
    système, points de restauration, santé des disques, énergie par application et MFT du disque
    système. Ni le modèle ni la page ne composent la liste : l'écran de revue permet seulement de
    décocher une tâche ou d'ajouter la MFT d'un autre disque NTFS du PC ;
  - protocole du helper en version 3 : une requête porte une tâche ou ce lot ; la réponse porte une
    section par tâche, avec son statut et sa couverture. Une tâche en échec ou hors délai n'annule
    pas les autres ;
  - budget du lot plafonné, mesuré avant d'être fixé ; l'expiration de la requête laisse le temps
    de lire l'écran de revue ; taille de réponse plafonnée par section ;
  - les résultats rejoignent le cache de `get_approved_elevated_inspection`, une entrée par tâche.
- Tâches, par ordre d'intérêt (ordre revu le 1er octobre : la MFT passe avant la santé des disques) :
  1. tailles des caches système (`Windows.old`, cache Windows Update, Optimisation de la
     distribution, points de restauration) : souvent les plus gros éléments récupérables, invisibles
     sans admin ;
  2. scan complet d'un volume par la MFT, selon le contrat de la feuille de route (S09) : tout C: en
     quelques secondes, avec des tailles sur disque exactes (liens physiques, fichiers compressés ou
     creux) testées sur des volumes de test avant tout conseil de suppression ;
  3. santé des disques (compteurs de fiabilité, SMART) ;
  4. chemins complets des processus protégés ;
  5. trace ETW de 30 à 60 s pendant le problème (latence DPC/ISR, E/S disque par fichier).
- Interface : un bouton « Analyse approfondie (administrateur) » dans Stockage, PC en direct,
  l'accueil du Chat et le premier lancement. Les tâches sont expliquées avant l'invite de Windows.
  Il est aussi proposé depuis le chat :
  - par une règle locale : quand un tour a reçu une couverture `needs_admin`, une carte « Lancer
    l'analyse approfondie » apparaît sous la réponse ; le modèle n'y a aucune autorité. Livré le
    18 octobre : le rapport porte `admin_hints` (vérifications manquantes, calculées dans
    `view.rs` à partir des couvertures `needs_admin` de `get_approved_elevated_inspection` et des
    caches système de `get_cleanup_opportunities`) ; la carte « Analyse approfondie disponible »
    sous la dernière réponse réutilise le flux de Stockage, puis propose de reposer la question ;
  - par le modèle : l'outil fermé `propose_elevated_check(tasks)` crée une proposition citée dans
    `action_refs` (C1) ; sa carte préremplit l'écran de revue, et rien ne se lance sans le clic
    local puis l'invite de Windows.
- La fenêtre et le LLM ne tournent jamais en admin, même pour le hackathon : un démarrage élevé est
  refusé avec un message visible, au lieu d'une fermeture silencieuse (#64).
- **Fait quand :**
  - la fenêtre et le LLM ne tournent jamais en admin ;
  - le protocole est testé : arguments hostiles, sortie tronquée, refus d'élévation ;
  - le lot est testé : une seule invite, une tâche en échec n'annule pas les autres, une
    proposition du modèle ne lance rien sans clic.

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
    `windowsupdate-optionalupdates` pour les pilotes proposés, `powersleep`) ;
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

### Chantier 13 — Optimiser le harness (piste A)

Mesuré avec les traces du chantier 7 ; chaque changement est gardé seulement si l'évaluation
s'améliore ou reste égale pour un coût moindre. Constat de départ (1er octobre, Super, réponse en
flux) : 6 requêtes et 56 500 tokens d'entrée pour une question, soit 0,023 $, et 8 passages non
vérifiés parce que le modèle recopie des valeurs brutes (octets, compteurs `_x100`).

- Sorties d'outils lisibles : chaque mesure donne sa valeur brute et sa forme lisible (« 2,1 Gio »,
  « 2 383 pages/s ») pour que le modèle cite des chiffres que le vérificateur reconnaît.
- Anti-appels redondants : un résultat identique à un appel récent n'est pas renvoyé en entier,
  seulement une référence à l'observation existante (déjà repéré avec Ultra au chantier 10).
- Contexte : les anciens résultats d'outils sont résumés au-delà d'un seuil ; les descriptions
  d'outils et le prompt système sont raccourcis sans perdre de règle ; cache de prompt si Nebius le
  propose.
- Budgets (requêtes, outils, durée, tokens de sortie) réglés d'après les traces, pas au jugé.
- **Fait quand :** pour un score égal ou meilleur, les tokens par réponse et les passages non
  vérifiés baissent nettement sur la campagne d'évaluation suivante.

### Chantier 14 — Escalade Super → Ultra (piste A)

- Super répond par défaut. Ultra reprend la même question, avec les observations déjà mesurées,
  quand l'évaluation montre que c'est rentable, par exemple :
  - des passages restent non vérifiés après les réparations ;
  - la confiance est faible ou la catégorie est « pas assez d'éléments » ;
  - le budget est épuisé sans réponse.
- Les seuils viennent de l'évaluation (chantier 7), jamais d'une intuition.
- L'interface dit qu'Ultra a repris la question et affiche le coût des deux modèles. Réglages :
  « automatique », « toujours Super », « toujours Ultra ».
- **Fait quand :** sur la campagne d'évaluation, la qualité approche celle d'Ultra seul pour un
  coût moyen nettement inférieur.

### Chantier 15 — Sources externes (pistes A et C ; E08, proposé le 3 octobre)

Démarre seulement si l'évaluation le justifie (chantier 7, scénarios « pilotes, mises à jour et
achats »). Contrat : C6. Nebius a annoncé le rachat de Tavily le 10 février 2026 : la soumission
montrerait deux briques de Nebius, Token Factory pour raisonner et Tavily pour chercher. Tavily
garde sa marque et sa propre clé API.

- **Pas de recherche en texte libre.** Les outils sont typés et leur requête est composée
  localement à partir des références émises par le lot 3 du chantier 4 :
  - nouvelle classe `ToolClass::External`, pour appliquer à part le consentement, le budget et
    l'affichage de la requête ;
  - nouveau fichier `crates/agent/src/external_tools.rs`, branché comme `system_tools.rs`.

| Outil | Contenu | Domaines |
| --- | --- | --- |
| `lookup_driver_notes(device_ref)` | dernière version du constructeur, problèmes ouverts et corrigés, bulletins de sécurité | nvidia.com, intel.com, amd.com, site du fabricant du PC |
| `lookup_known_issue(ref)` | blocage de compatibilité : page release health officielle téléchargée, identifiant cherché localement, sans requête de recherche ; code d'erreur, KB ou module en cause : recherche | learn.microsoft.com, support.microsoft.com, sites des constructeurs |
| `lookup_upgrade_price(component)` | `memory` d'abord : fourchette de prix datée pour une barrette compatible (type, format, vitesse, capacité), dans le pays de la région Windows | sites de vente et comparateurs du pays, liste à fixer |
| `lookup_pc_specs()` | en dernier : fiche constructeur du modèle (RAM maximale, soudée ou non, emplacements M.2) | site du fabricant du PC |

- Client `crates/agent/src/tavily.rs`, sur le modèle de `nebius.rs` (https seulement, délais,
  aucune donnée personnelle dans les en-têtes) :
  - `search_depth: basic`, `include_domains` en mode `restrict`, 5 résultats au plus ;
  - dates de publication demandées, `include_usage` pour le coût ;
  - jamais `include_answer` (réponse invérifiable d'un autre LLM), ni `include_raw_content`, ni
    `extract` sur une URL fournie par le modèle.
- Cache dans `crates/store` : 24 h pour les prix et les notes de pilotes, 7 jours pour les
  problèmes connus. Plafond de recherches par tour, réglé d'après les traces. Le coût Tavily
  s'ajoute à celui du tour
  (tarifs relevés le 3 octobre : 1 crédit par recherche basic, 1 000 crédits gratuits par mois,
  puis 0,008 $ le crédit).
- Sans clé ou sans consentement : statut `unsupported` ou `denied`, et tout le reste fonctionne.
  Pour le jury, la clé Tavily reste facultative.
- Consignes du prompt (piste A) :
  - les résultats web ne sont pas fiables, comme toute sortie d'outil ;
  - une correspondance avec un problème connu est un indice, pas une preuve : confiance moyenne
    et formulation prudente ;
  - un prix est une fourchette datée et sourcée ;
  - la dernière version d'un constructeur n'est proposée que si ses notes corrigent le symptôme
    mesuré ou une faille.
- Interface (piste C) :
  - cartes de source pour `[W1]` : titre, domaine, dates ;
  - lien ouvert dans le navigateur, avec une URL résolue côté Rust à partir de la citation, en
    https seulement ;
  - bascule « Recherche web » dans la zone de saisie (C5), clé Tavily dans les réglages.
- **Décidé le 9 octobre :** pas de confirmation de la première recherche ; la bascule mémorisée
  fait foi, active par défaut dès qu'une clé existe.
- **À vérifier :**
  - les conditions de Tavily, avant d'écrire dans l'app la « zero data retention » annoncée par
    sa FAQ ;
  - la fiabilité de `published_date`, encore en bêta chez Tavily ; une date absente reste
    inconnue ;
  - les sites de vente et comparateurs par pays, et la qualité des extraits de prix ;
  - les adresses des pages release health par version de Windows, et la présence des identifiants
    de blocage dans leur texte ;
  - la capacité Tauri minimale pour ouvrir un lien externe ;
  - le coût et la latence réels d'une recherche, mesurés sur le PC de dev.
- **Fait quand :**
  - des tests sur réponses synthétiques, y compris des extraits piégés, ne montrent aucun
    changement de permission, aucune action et aucune fuite ;
  - une recherche réelle a été faite sans donnée sensible ;
  - la requête affichée est identique octet pour octet à celle envoyée ;
  - les captures du chantier 7 enregistrent les résultats web, et le rejeu ne relance aucune
    recherche.

## 5. Calendrier

| Semaine | Travail |
| --- | --- |
| **S1 · 28 sept.–4 oct.** | Contrats C1 à C5 (jours 1 et 2). Chantier 2. Capture, rejeu et ligne de base A et D sur 5 scénarios. Chantier 1 : prompt, `submit_answer`, vérificateur. Maquette de conversation sur données simulées. |
| **S2 · 5–11 oct.** | Priorité : `pcagent-eval` (capture, rejeu, traces réelles) et chantier 13. Chantier 1 : multi-tour et stockage SQLite. Chantier 4, lot 1 : activité, démarrage, services, événements, alimentation. Chantier 3. Fil et historique branchés. Évaluation 1 (A à D, 10 scénarios). Chantier 4, lot 3 : SMBIOS, événements, blocages de compatibilité. PR du plan pour le contrat C6. Scénarios « pilotes, mises à jour et achats ». Ajouté le 6 octobre à la demande du porteur du projet : chantier 6, phase 2, Stockage en carte, arbre et liste, Historique minimal et Surveillances (C7), sur un socle commun de barre secondaire. |
| **S3 · 12–18 oct.** | Chantier 14 (escalade Super → Ultra). Chantier 6, phase 2 : rail, PC en direct, inspecteur avec Expliquer et Enquêter. Chantier 4, lot 2 : applications, nettoyage, inventaire, `query_system`. Chantier 5. Refonte visuelle et panneau de droite. Chantier 8, tâches 1 et 2 (caches système, MFT). Évaluation 2. Chantier 4, lot 3 : `get_drivers` et Windows Update. Chantier 15 si l'évaluation le justifie : notes de pilotes et problèmes connus. |
| **S4 · 19–25 oct.** | Chantier 6, phase 2 : validation dans l'app Windows et finitions de Stockage, Historique et Surveillances (démarrés en S2). Chantier 11. Chantier 8, tâches 3 à 5 si le temps le permet. Chantier 9. Réponse en flux. Performance et coût. Second PC. Évaluation 3. Chantier 15, suite : prix, puis fiche constructeur. |
| **S5 · 26–30 oct.** | Gel le 27. Corrections. README, docs, licence. Vidéo. Release. Soumission avant le 30 octobre à 18 h. |

Repoussé après la soumission :
- Surveillances, suite (C7 couvre les règles, les notifications et les alertes avant la
  soumission) : bilan hebdomadaire local ou sur un clic (aucune requête cloud automatique),
  Expliquer une alerte (référence de sélection, complément de C3) et outil `get_watch_alerts` ;
- historique au-delà de 7 jours et enregistrement automatique quand un ralentissement est détecté ;
- spécialistes (un ou deux agents dédiés, par exemple stockage ou jeu), seulement si l'évaluation
  montre des familles de tâches réellement séparables ;
- LoRA ou distillation d'un modèle NVIDIA plus petit sur des traces de qualité consenties et
  anonymisées, qui pourrait à terme tourner en local ;
- index persistant et journal USN (S10) ;
- export expurgé (U01) ;
- coffre Windows pour la clé (U04) ;
- fluidité en jeu mesurée (E06) ;
- réseau (E07) ;
- recherche documentaire libre (E08) ; le chantier 15 n'en avance que des outils typés ;
- alertes au-delà des règles C7 (E09) ;
- antivirus (E10) ;
- corruption (E11) ;
- doublons étendus (S11) ;
- capteurs par le pilote de LibreHardwareMonitor dans le collecteur élevé (chantier 9) ;
- fournisseurs de modèles hors NVIDIA, compatibles OpenAI ou API Anthropic (chantier 10).

## 6. Répartition entre Claude et Codex

Les pistes touchent des fichiers différents pour limiter les conflits.

| Piste | Chantiers | Fichiers principaux |
| --- | --- | --- |
| A — moteur | 1, 3 (envois), 10, 11 (moteur), 15 | `crates/agent`, `crates/store`, `apps/cli` |
| B — collecte et outils | 2, 4, 5 (collecte), 8, 9 | `crates/telemetry`, `crates/inspect`, `crates/agent/src/system_tools.rs`, `apps/elevated` |
| C — interface | 6, 3 (écrans), 5 (bouton), 11 (cartes), 15 (sources et réglages) | `apps/gui` |
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
7. **Interface (1er octobre) :** la conversation d'abord, avec un panneau de contexte fermé par
   défaut qui s'ouvre depuis une citation, l'état du PC ou le stockage. Accent unique Encre
   (`#3B5BDB`), Iris réduite à une marque fixe, coût affiché sous chaque réponse. La refonte garde
   le JavaScript sans build : aucun framework n'a été nécessaire.

## 7 bis. Priorités ajoutées (1er octobre)

1. Mesurer d'abord : `pcagent-eval` et les traces réelles passent avant tout nouvel agent.
2. Optimiser le harness (chantier 13), puis l'escalade Super → Ultra (chantier 14).
3. Spécialistes, LoRA et distillation seulement après la soumission, et seulement si l'évaluation les
   justifie.

## 7 ter. Sources externes (3 octobre)

1. Un pilote se juge sur trois versions : celle installée, celle que propose Windows Update
   (l'option prudente, souvent en retard sur NVIDIA, Intel ou AMD) et la dernière du constructeur,
   avec ses notes. « N'y touchez pas » reste une réponse valable.
2. Les clés de recherche locales passent d'abord (chantier 4, lot 3) : elles servent même sans web.
3. Pas de recherche web en texte libre avant la soumission : seulement des outils typés dont la
   requête est composée localement (C6).
4. Tavily est la source web retenue, avec une clé facultative. Le chantier 15 démarre seulement si
   l'évaluation montre un manque d'informations récentes.

## 7 quater. Retours du 9 octobre

Revue de l'interface et retours du porteur du projet, suivis dans les issues #50 à #71 (suivi #72).

1. **Partage :** aucun dialogue avant l'envoi. Des bascules dans la zone de saisie, actives par
   défaut (noms des programmes, noms de fichiers, recherche web), et l'aperçu exact à la demande.
   La liste de ce qui ne part jamais ne change pas (C5, C6, chantier 3 ; #54).
2. **Admin :** la fenêtre et le LLM restent sans admin. Un lot fermé passe sous une seule invite
   UAC et peut être proposé depuis le chat, par une règle locale ou par le modèle (chantier 8, C1,
   C3 ; #62).
3. **Expliquer et Enquêter :** deux boutons, affichés « Résumer ces mesures » et « Chercher la
   cause ». Le résumé s'affiche dans l'inspecteur ; « Chercher la cause » continue la même
   conversation (chantier 6, phase 2 ; #56).
4. **Branches :** `plan-mockups` (compléments C3 du 6 octobre et C7) et `wp6-viewbar` (barre
   secondaire par vue) sont fusionnées.
5. **Orientations, chacune à détailler dans sa propre PR du plan :** le type des processus d'un
   navigateur est dérivé localement de la ligne de commande, qui n'est ni gardée ni envoyée (C4,
   C5 ; #60) ; l'historique gagne une lecture réduite et une table compacte des valeurs système
   (C2, C3 ; #58, #59) ; les images jointes sont repoussées après la soumission, sauf si Nebius
   sert un modèle NVIDIA qui accepte à la fois les images et les outils (#55).

## 7 quinquies. Plan de la suite (17 octobre)

Échéance Devpost : **vendredi 30 octobre, 10 h PDT (18 h Paris)** ; soumission visée le 29 pour
garder une journée de marge. Jury du 1er au 15 décembre : la build soumise doit rester gratuite,
accessible et stable jusque-là. Le règlement n'impose aucun système : Windows 10/11 x64 est un
choix produit, annoncé tel quel dans le README.

Règle de conduite jusqu'au 30 : on ne touche pas aux contrats C1–C7 et on ne commence aucun nouveau
chantier ; tout ce qui n'est pas dans cette section est repoussé.

**Décision du porteur du projet, 26 octobre (remplace la règle ci-dessus et le gel du 27) :** tout
terminer avant l'échéance, y compris les listes « Repoussé après la soumission » et la phase 5.
Chaque élément entre dans `main` dès qu'il est entièrement validé (formatage, Clippy
`-D warnings`, tests sous un compte standard, build release) et sans déstabiliser la build
soumise, jusqu'au 29 octobre ; les éléments de la soumission passent en premier. Les contrats
C1–C7 ne changent toujours que sur décision du porteur, dans une PR du plan à part. Le code est
nettoyé et refactorisé au passage, sans changer son comportement ; la restructuration du dépôt est
proposée au porteur avant tout déplacement de fichiers. Le 29 octobre (Paris) : gel, tag annoté
`v0.1.0` sur `main` et release GitHub avec les quatre exécutables (Devpost renvoie au tag). Du
30 octobre 10 h PDT au 15 décembre, `main` est gelée (règlement Devpost : aucune modification de la
soumission après la période de soumission) ; ce qui n'est pas terminé va sur une branche `next`,
fusionnée dans `main` après le 15 décembre.

### Phase 0 — Remise en route (17–18 oct.)

| Tâche | Pourquoi | Fait quand |
| --- | --- | --- |
| Réactiver la CI GitHub Actions et la remettre au vert (Ubuntu + Windows) | Derniers runs en échec, CI désactivée depuis le 10 | `main` vert sur les deux OS |
| Fermer les issues déjà traitées (#50–#54, #57, #68 ; #56 après vérification) | Le suivi #72 reflète l'état réel | issues fermées avec renvoi à la PR |
| Décider du sort de la branche `wp8-mft-memory` (fusion ou suppression) | Une seule branche durable | branche fusionnée ou supprimée |
| Scanner tout l'historique git à la recherche de secrets (gitleaks) | Pré-requis avant de rendre le dépôt public | rapport vide, ou secrets révoqués et purgés |

### Phase 1 — Bloquants produit (19–22 oct.)

| Tâche | Issue | Fait quand |
| --- | --- | --- |
| L'app lancée en admin affiche un message clair au lieu de se fermer | #64 | testé en admin et sans admin |
| Onboarding du premier lancement : lien pour créer une clé Nebius, test de la clé, choix du modèle, mode local sans clé | #63 | parcours complet sur un profil vierge |
| Langue : textes d'interface en français, formats de nombres et unités (Gio) | #65 | aucune chaîne anglaise ni format Rust dans l'UI FR |
| Vouvoiement fixé dans les réponses en français | #66 | vérifié sur les scénarios d'évaluation |
| Parcours complet avec une vraie clé dans l'app Windows (question → outils → réponse citée → action validée) | section 8 | enregistré une fois, sans erreur |
| Registre des capacités : chaque outil et chaque vue déclare s'il est disponible (admin, NVIDIA, MFT…) ; le modèle et l'UI ne voient que ce qui marche | — | outils indisponibles absents du catalogue envoyé, tests synthétiques |

### Phase 2 — Évaluation réelle (en parallèle, 19–24 oct.)

| Tâche | Fait quand |
| --- | --- |
| Capturer 8 à 10 vraies situations (mémoire, Chrome, stockage plein, démarrage, plantage, PC sans problème, pilote NVIDIA…) | scénarios anonymisés dans `eval/scenarios/` |
| Campagne B (Nemotron, C1) contre C (Claude via MCP), notation à l'aveugle par le porteur du projet + juge LLM | `docs/eval.md` daté, scores figés |
| Choisir le modèle par défaut (chantier 10) et décider si l'escalade Super → Ultra reste manuelle | décision écrite dans ce plan |
| Si l'escalade est gardée : la rendre visible et afficher le coût des deux modèles | #69 fermé |

Cet ordre est voulu : la vidéo et le README citeront ces résultats (« Technological
Implementation » est un critère de jugement).

### Phase 3 — Dernières fonctions et second PC (23–26 oct.)

| Tâche | Fait quand |
| --- | --- |
| Vue Stockage **minimale** : résultats visibles en premier, liste et inspecteur ; carte et arbre seulement si le temps le permet | #67 partiellement traité, le reste en P2 |
| Processus msedgewebview2 distingués par application | #61 |
| Test complet sur un second PC Windows 11 (installation depuis la release, clé, parcours vidéo) | checklist cochée |
| Mesurer l'empreinte de l'app (RAM, CPU, disque de l'historique) pour le README | chiffres datés |

**Repoussé après la soumission :** Surveillances (C7), vue Historique complète, #58/#59 au-delà du
premier lot, #60 (navigateurs), #62 (lot UAC unique), #55 (images), ETW, validation des capteurs
actifs (chantier 9), cache Tavily.

### Phase 4 — Gel et soumission (27–29 oct.)

1. **Gel le 29** (décision du 26 octobre, qui lève le gel du 27) : tag annoté `v0.1.0` sur `main`
   et release ; ensuite, seulement des corrections de bugs jusqu'à la soumission.
2. README en anglais : vision, captures, installation, plateforme annoncée (**Windows 10/11 x64**),
   confidentialité, usage de Nemotron et de Token Factory, avertissement SmartScreen.
3. Docs en retard sur le code (#71) : README, `architecture.md`, `validation.md`.
4. Release GitHub sur le tag `v0.1.0` : `pcagent-gui.exe`, `pcagent-elevated.exe` (à placer à côté
   de la fenêtre), `pcagent.exe` et `pcagent-eval.exe`, version et commit.
5. Rendre le dépôt public, licence MIT visible dans « About ».
6. Vidéo de moins de 3 minutes en anglais, publique sur YouTube : vrai problème → conversation →
   outils en direct → réponse citée → action validée → avant/après.
7. Formulaire Devpost : piste, description, instructions de test (chaque juge apporte sa clé
   Nebius, ou mode local), retours sur Token Factory et Nemotron, ce qui a changé pendant la
   période de soumission.

### Phase 5 — Multi-OS (avancée avant l'échéance, décision du 26 octobre)

Ne pas casser la release soumise pendant le jugement (1er–15 déc.) : le travail multi-OS se fait sur
des branches et de nouvelles versions, dans le même dépôt et le même workspace (trois cibles de
build, pas trois forks). Décision du 26 octobre : ces étapes sont menées avant le 29 octobre, chacune
fusionnée dans `main` une fois validée et sans déstabiliser la build soumise ; ce qui reste au
30 octobre va sur `next`.

1. PR du plan : contrats renommés de façon neutre (commit, startup, services, events, settings
   pages), registre des capacités étendu par OS.
2. Refactor `os/{windows,linux,macos}` derrière un trait commun dans `telemetry`, `inspect`,
   `storage` et `actions` ; les fonctions Windows restent réservées à Windows (MFT, élévation, ETW,
   Windows Update).
3. Backend Linux (`/proc`, `/sys`, systemd, journald, XDG autostart) avec la CI Ubuntu.
4. GUI Tauri sur les trois OS, pilotée par le registre des capacités.
5. Backend macOS (libproc, host_statistics, IOKit, launchd), testé sur un vrai Mac, avec un job CI
   macOS.
6. Reprendre ce qui a été repoussé en phase 3, par ordre de priorité.

### Décisions encore ouvertes

- Piste Devpost : « Best Apps and Agents » (celle de la spec v0.1) ou « Personal AI ».
- Vue Stockage minimale avant le gel, ou coupée complètement.
- Dépôt public dès la phase 0, ou seulement le 28 octobre.

## 8. Suivi

**19 octobre 2026 — essai C1 / #85 :** exception de correction finale autorisée par le porteur,
définie ci-dessus avant l'implémentation dans une PR distincte. À mesurer sur des parcours Nebius
réels avant adoption ; le choix nommé ne constitue pas une garantie de latence ni de réponse unique.

| Chantier | Piste | État | PR |
| --- | --- | --- | --- |
| 0 — Plan, archives, licence | — | fait | #18 |
| Contrats C1–C5 | toutes | implémentés côté backend ; intégration des écrans C3/C5 à terminer au chantier 6 | wp1-phase1-logic |
| 1 — Le LLM rédige | A | logique terminée : réponse, vérification des mesures C4, contexte multi-tour, stockage et identifiants stables, flux et arrêt testés ; comparaison de qualité à faire au chantier 7 ; #66 : consigne de vouvoiement ajoutée, évaluation à réaliser | #19, #23, wp1-stream, wp1-phase1-logic |
| 2 — Tous les processus, sans admin | B | logique terminée : fenêtres, regroupement, services et GPU par processus ; couverture et coût mesurés sans admin (voir docs/telemetry.md) | #22, wp1-phase1-logic |
| 3 — Choix de partage | A, C | logique terminée : trois modes persistants, aperçu figé, retrait des noms des suivis, noms de fichiers sur liste finie et filtre C5 ; bascules de la zone de saisie (noms, web, dossier joint, noms de fichiers), actives par défaut et mémorisées, « Voir ce qui part », plus aucun dialogue avant l'envoi, message exact gardé dans les étapes du tour (10 octobre) ; à vérifier dans l'app Windows avec une vraie clé | #21, #23, wp1-phase1-logic, #54, wp3-composer-toggles |
| 4 — Nouveaux outils | B | logique terminée : outils fermés, collecte native bornée, couverture et projections C4/C5 ; tests synthétiques et traces locales (voir docs/system-inspection.md) | wp1-phase1-logic |
| 5 — Mesurer au bon moment | B, C | logique terminée : historique 7 jours, agrégation, lacunes, pics et enregistrement, démarrage optionnel et empreinte mesurée ; contrôles d'enregistrement, historique et réglage de démarrage à brancher dans le frontend | wp1-phase1-logic |
| 6 — Interface de conversation | C | interface existante conservée ; commandes Rust Expliquer/Enquêter, sources figées et aperçu exact livrés dans le lot backend ; branchements de phase 2 à terminer (docs/selection.md) ; #65 : formats localisés sur les champs numériques, résumé scanné désormais couvert | #23, #24, #36 |
| 7 — Évaluation | D | capture/rejeu/traces et compilateur local d'étalonnage Nemotron livrés ; traces réelles Super/Ultra conservées ; annotation humaine et campagne de continuation hybride à réaliser, comparaisons externes exclues de ce lot | #33, #36 |
| 8 — Admin à la demande | B | helper, caches, MFT natif borné avec arbre local, VSS, fiabilité disques, identités de processus et estimations E3 livrés ; ETW préparé mais bloqué avant effets tant que le nettoyage après arrêt brutal n'est pas prouvé ; acceptation UAC/second PC à réaliser ; carte par règle locale sous la réponse du chat livrée le 18 octobre ; lot sous une seule invite et proposition par le modèle (`propose_elevated_check`, 9 octobre) à faire | #33, #36, #62 |
| 9 — Capteurs matériels | B | lecteurs passifs LHM/HWiNFO livrés ; validation avec fournisseur actif à réaliser | #33 |
| 10 — Modèle | A | catalogue NVIDIA et métadonnées livrés ; modèle par défaut conservé, décision fondée sur l'évaluation encore attendue | #33 |
| 11 — Agir depuis la conversation | A, C | démarrage HKCU, restauration liée au journal, TEMP/shaders et pages Paramètres livrés ; acceptation native sur cibles contrôlées à réaliser | #33 |
| 12 — Soumission | D | planifié le 17 octobre (section 7 quinquies, phases 0 à 4) | |
| 6, 12 — Premier lancement | C | réalisé localement : guide Nebius reprenable, catalogue du compte, test explicite et mode local ; saisie masquée clarifiée avec accord du porteur ; tests JS/Rust et build Windows validés ; QA native non-admin et catalogue/test Nebius réels validés, révélation WebView2 et casse des modèles du mock corrigées ; ouverture du portail non validée dans le bureau isolé ; PR vers main, contrats C1–C7 inchangés | #63, wp12-onboarding |
| 13 — Optimiser le harness | A | formes lisibles, réutilisation des résultats identiques et compaction livrées ; #84 : réparations regroupées, lecture SSE complète et borne de coût uniquement à tarif vérifié ; exception C1 définie dans #86 : outil final nommé en correction après mesures utilisables, validé par un test live contrôlé (1 appel accepté, 61 s) ; 745 tests locaux passent ; #85 reste ouvert : campagne du 19 oct. sur main (41 appels réels) : la réponse finale est complète en 18–45 s puis le modèle continue 70–90 s (balises, seconde copie) jusqu'au plafond ; raisonnement léger sans gain ; correctif wp13-stream-early-close : fermeture du flux dès que l'appel final nommé est complet (usage inconnu pour cette requête, coût en borne basse), fusionné le 26 oct. après validation locale (formatage, Clippy, 746 tests sous compte standard, build release) ; gain de latence en parcours réel encore à mesurer ; C2–C7 inchangés | #33, wp13-agent-reliability, #84, #85, #86, #89 |
| 14 — Escalade Super → Ultra | A | mécanisme backend livré : reprise unique, budgets et preuves conservés, coût par modèle, réglages locaux ; automatique désactivé sans étalonnage humain réel compatible avec outils/moteur/harness ; qualité hybride à valider | #34, #35, #36 |
| 4, lot 3 — Clés de recherche locales | B | côté backend sur main : SMBIOS (type, format, capacité), champs crash/bugcheck/KB des événements, `get_drivers` et offres Windows Update ; validation à faire | |
| Contrat C6 — Sources externes | A, C | adopté avec cette PR du plan | #31 |
| 15 — Sources externes | A, C | livré : client Tavily, quatre outils typés, citations `[W…]` vérifiées, consentement et clé ; cache et pages release health à faire | #44 |
| 6 — Batterie dans PC en direct | B, A, C | livré : lecture batterie dédiée hors worker partagé, ressource de sélection `battery`, carte avec barre d'action, inspecteur, panneau et pastille ; vérifié sur un portable en charge ; décharge, enregistrement avec la vue ouverte et durée de lecture à mesurer | wp6-live-battery |
| 6 — Stockage en carte, arbre et liste | B, A, C | à faire, aucune branche ouverte au 9 octobre : complément C3 du 6 octobre (C4, C5), sur le socle `wp6-view-sidebar` ; hiérarchie et faits dans `crates/storage`, faits figés dans la sélection, comptes lus par le vérificateur, barre secondaire des disques et analyses récentes, carte, arbre, liste et inspecteur | wp6-storage-map |
| 6 — Vue Historique | C, A | à faire, aucune branche ouverte au 9 octobre : complément C3 du 6 octobre (`history_overview`, `recording_detail`), sur le socle `wp6-view-sidebar` ; barre secondaire, frise d'événements numérotés, inspecteur et rechargement mesuré | wp6-history |
| 6 — Surveillances | B, A, C | à faire, aucune branche ouverte au 9 octobre : contrat C7 adopté, sur le socle `wp6-view-sidebar` ; règles, moteur dans l'échantillonneur avec bail, notifications Windows, vue et bouton Surveiller | wp6-watches |
| 6 — Barre secondaire par vue | C | livré : une vue peut déclarer sa barre dans l'emplacement de la liste des conversations (`shell.setViewBar`), redessinée sur ses sujets, avec repli, fenêtre étroite, focus et repère accessibles ; aucune vue ne l'utilise encore (Stockage, Historique, Surveillances) | wp6-viewbar |
| Retours UI/UX du 9 octobre | A, B, C | 22 issues ouvertes (18 en P1 avant le gel, 4 en P2) ; décisions en section 7 quater ; #50 à #54, #57 et #68 traités côté interface le 10 octobre (branches `wp6-chat-polish`, `wp6-views-polish`, `wp3-composer-toggles`) ; reprise terminée : corrections de revue, confidentialité et envois concurrents, 52 contrôles navigateur et 9 contrôles natifs locaux réussis, release reconstruite sur D:, formatage/Clippy et 715 tests réussis (2 ignorés) ; parcours avec modèle réel encore à vérifier | #72 |
| 6 — Résumé dans l'inspecteur et recherche de cause | C | implémenté et validé localement (10 octobre) : #56, résumé sans quitter la vue, poursuite du même fil et étiquettes de sélection ; 57 contrôles rendus, 28 régressions du contrôleur et 12 contrôles natifs réussis ; formatage/Clippy et 715 tests réussis (2 ignorés), binaire reconstruit sur D: ; évaluation réelle des réponses et livraison reportées à la demande du porteur du projet | wp6-selection-inspector |
| 5, 6 — Historique : lectures et volume | B, C | premier lot implémenté et validé localement (10 octobre) : format compact compatible en lecture avec les anciennes mesures, lecture partagée et cache invalidé après écriture, chargement et tracé réduits préservant lacunes, extrema et couvertures ; formatage/Clippy, 732 tests Rust (2 ignorés), 42 contrôles navigateur et 12 contrôles natifs réussis, CLI et GUI release reconstruits sur D: ; mesures et limites dans [history-performance.md](history-performance.md) ; #58/#59 restent partiels : commande dédiée, table système, comptes d'événements et budget disque demandent un lot de contrats distinct | wp5-history-performance |
| 2 — WebView2 par application (#61) | B | fait (26 oct.) : processus `msedgewebview2` rattachés à leur application hôte par la chaîne des parents validée (jamais par le nom d'image), sans ligne de commande ; un arbre sans hôte attribuable forme son propre groupe ; 3 tests synthétiques (dont un contrôle par mutation) ; fusionné après validation locale (formatage, Clippy, 749 tests sous compte standard, build release) ; pas encore vu avec de vrais hôtes WebView2 (aucun sur la VM) | wp2-webview2-groups |
| 14 — Escalade visible (#69) | A, C | en cours (26 oct.) : changement de modèle affiché sur la réponse, coût par modèle dans l'app et la CLI | wp14-escalation-visible |
| 6 — Stockage minimal (#67) | C | fait (26 oct.) : résultats d'abord (totaux, liste), inspecteur de l'élément sélectionné à côté de la liste ou sous la ligne quand la fenêtre est étroite, cartes admin regroupées après les résultats ; 11 tests Node, tour en/fr clair/sombre sans erreur ; carte et arbre restent en P2 | wp6-storage-minimal |
| 2 — Noms lisibles des programmes (#70) | B/C | fait (26 oct.) : `ProgramNames` (FileDescription puis ProductName de la ressource de version, cache par chemin, lecture sans admin), nom lisible dans PC en direct, l'inspecteur, les réponses, les constats, les sources et les revues d'action ; exécutable en infobulle, PID seulement dans l'inspecteur ou pour distinguer deux processus ; jamais envoyé au modèle (C5) ; fmt, Clippy, tests, tour | wp2-app-names |
| 14 — Escalade visible et coût des deux modèles (#69) | C | fait (26 oct.) : étape « Ultra reprend la question », en-tête « Super → Ultra · total », note durable sous la réponse avec la raison (et la vérité quand Ultra n'a rien écrit), une ligne par modèle dans Sources › Requêtes et coût, même détail dans la CLI ; jamais le prix d'un modèle appliqué aux jetons de l'autre ; Auto proposé dès qu'une calibration relue existe ; fmt, Clippy, tests, tour | wp14-escalation-visible |
| 6 — Interface en français (#65) | A, C | fait (26 oct.) : phrases du cœur envoyées comme clés fermées à valeurs typées (`Phrase`, `phrases.js` : constats, étapes suivantes, limites, faits des sources, réponse locale, GPU, couverture admin, processus sans nom) et erreurs des commandes avec code fermé traduit (`failure.rs`, `failure.js`), le texte anglais restant la référence pour le modèle, la CLI et le vérificateur ; menu de la barre d'état et dialogue de dossier dans la langue de Windows ; tests, tour FR | wp6-french-ui, wp6-french-errors |
| Q3 — Interface organisée par fonctionnalité | C | implémenté et validé (26 oct.), en attente de fusion : `chat/`, `selection/`, `settings/`, `live/`, `storage/`, `elevation/`, `history/` ; décisions de santé séparées du DOM ; 40 modules liés sans exécution, 56 tests Node, fmt, Clippy, 16 exécutables de test sous compte standard, doctests et release des 4 programmes ; pas de changement visuel, parcours natif non rejoué dans ce lot | wp6-ui-feature-modules |
| Registre des capacités (phase 1) | A, B, C | fait (26 oct.) : `crates/agent/src/capabilities.rs` (9 capacités, table REQUIREMENTS, variantes constantes), outils indisponibles absents du catalogue envoyé et du prompt système, `app_info.capabilities` pilote la vue Stockage et la carte des vérifications ; 224 tests agent, tour ; identité de calibration et replays inchangés | wp4-capability-registry |
| 12 — README et docs (#71) | D | fusionné (26 oct., `4b341ff`) : README en anglais avec captures du mock, `architecture.md`, `validation.md`, `.env.example` ; restent trois TODO liés à la release (lien de la release, empreinte mesurée, résultats de l'évaluation) | wp12-docs |

Arrêt demandé le 10 octobre après la validation de ce lot, avant la nouvelle vue Stockage et sans
commencer un autre chantier. L'évaluation réelle des réponses et la livraison restent reportées.
La CI GitHub Actions est temporairement désactivée à la demande du porteur du projet ; les
contrôles ci-dessus ont été exécutés localement. Aucune fusion vers `main` n'est effectuée.

Le lot de corrections des chantiers 1, 3 et 6 distingue les limites couvertes, les choix volontaires
et les écarts restants dans [answer-verification.md](answer-verification.md). Les parcours testés
sont consignés dans [gui.md](gui.md#validation-status). Les tests du contrôleur et du GUI ne
valident pas à eux seuls la qualité du modèle ; celle-ci reste à annoter sur les situations réelles.

Les contrats de phase 1 sont détaillés dans [backend-phase1.md](backend-phase1.md). Le prolongement
backend du 4 octobre est décrit dans [backend-completion.md](backend-completion.md), avec ses
commandes, preuves de validation et acceptations restantes. Les fichiers frontend et la
configuration des clés restent hors du périmètre de ce lot.
