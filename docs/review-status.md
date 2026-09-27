# PC Agent — améliorations produit, limites et suivi de revue

**Mise à jour : 27 septembre 2026.** Ce document rassemble le travail restant identifié par lecture du code, de la [spécification](../SPEC_PC_AGENT.md), de la revue initiale et de sources externes. Il décrit des recommandations, pas des fonctionnalités livrées ni une promesse de calendrier. Le document original `pc-agent-review.md` est conservé.

**Ma recommandation : terminer le parcours « je reproduis ma lenteur → le logiciel observe → il explique → je choisis une suite → il vérifie ».** Les investissements les plus utiles sont l'observation sur une vraie durée, des conclusions adaptées à la question, un parcours stockage orienté objectif et une validation sur d'autres machines. Une longue liste de capteurs ou d'actions ne remplace pas ces parcours.

Lecture rapide : [état réel du code](#etat-du-code), [enseignements de la comparaison](#comparaison), [prochain lot recommandé](#prochain-lot), [backlog détaillé](#backlog), [ordre de livraison](#livraison), [limites à conserver](#limites-produit), [sources externes](#sources-recherche). La revue technique historique est conservée plus bas.

<a id="etat-du-code"></a>

## État du code pris en compte

**Mise à jour du 27 septembre 2026 :** `main@536e382` intègre les évolutions NVML/coûts et
prolongation/citations/statuts. Les deux références ci-dessous décrivent la lecture initiale,
avant cette fusion. Le lot « contexte stockage » part de ce `main` intégré et apporte :

- des alias distincts fichiers/dossiers, reliés aux vrais chemins dans la CLI et le JSON local ;
- le rôle et l'application suggérés par les règles locales, avec leur base et leurs conditions,
  sans prétendre vérifier l'installation ou la propriété ;
- une comparaison locale de contenu SHA-256 avec `--compare-content`, bornée et facultative,
  qui affiche des copies observées sans les supprimer ;
- `--share-file-names`, indépendant des lectures de contenu : liste finie de noms utiles,
  aperçu `--preview-cloud`, affichage de la liste réelle avant envoi, aucune extension implicite
  des noms partagés après un nouveau scan ;
- le pré-scan borné des `--scan-root`, réutilisable immédiatement par l'agent.

**Wanted limitations :** noms masqués et métadonnées seules par défaut ; autorisations séparées
pour lecture de contenu, noms et cloud ; aucune suppression nouvelle ; contenu et empreintes
jamais envoyés. **Covered limitations :** rôles/applications déduits de règles de chemin,
restauration/corbeille/sauvegardes non vérifiées, comparaison partielle et non atomique, identités
liées à un scan, délais natifs coopératifs. Les fichiers non comparés ne sont pas déclarés uniques.
La comparaison concerne au plus 256 fichiers par scope ; le partage au plus 200 noms par run.
Les détails et commandes sont dans [storage.md](storage.md#context-names-and-content-comparisons).
Restent ouverts : sélection fine des noms à partager, navigation S02, attributs S03, comparaison
entre scopes, recherche exhaustive de doublons, restauration réellement testée, antivirus et
validation de formats corrompus. Ces derniers ne sont pas inclus dans ce lot.

La lecture porte sur **deux états distincts**, et non sur une version imaginaire contenant déjà toutes les évolutions :

| Référence examinée | Déjà présent | Ce qui reste à vérifier |
|---|---|---|
| `main` au commit [`92b7a28`](https://github.com/nassim-arifette/pc-agent/commit/92b7a285e3d21d4c34ab059a2cd49ec5278f1c54) | Prolongation consentie du budget ; citations d'observations matériel/stockage/historique ; séparation `execution_status`, `coverage`, `diagnostic_support` ; modules budget/rapport/tests séparés. | Préserver ces changements lors de l'intégration des autres travaux. La prolongation existe déjà : ce n'est plus une fonctionnalité à construire. |
| Branche `nvml-cost-tokens` au commit `0528675`, incluant `03319fc` et `d266da4` | Tokens et estimation de coût datée ; contexte modèle compact et alias ; capteurs NVIDIA/NVML et findings associés. | Au moment de la lecture, cette branche n'intègre pas `92b7a28`. Les coûts, nouveaux capteurs, citations, couverture et prolongations doivent être validés ensemble. |
| Socle commun et documentation des crates | Collecteurs CPU/mémoire/E/S, historique système en mémoire, scanner progressif, politique de classement, plans bornés, approbation locale, journal et vérification selon l'action. | Une capacité présente dans une crate n'est pas nécessairement accessible dans le parcours CLI/modèle, ni validée sur un autre PC. |

Les documents de la branche de travail peuvent donc encore décrire des limitations déjà levées sur `main`. Le backlog ci-dessous ne redemande ni les trois dernières corrections, ni l'ajout initial de NVML, ni le suivi initial des tokens/coûts.

**Méthode et limites de cette revue.** Lecture du CLI, agent, collecteurs/historique, diagnostics, stockage, politique, actions et CI ; recherche documentaire et témoignages qualitatifs. Aucun test live Nebius, benchmark concurrent, entretien utilisateur ou nouvelle recette Windows n'a été exécuté pour rédiger ce document. Les résultats de tests historiques sont attachés à leur lot, pas transposés à une future fusion. Les fonctionnalités externes sont celles décrites par leurs auteurs, pas des performances mesurées ici.

<a id="comparaison"></a>

## Ce que la comparaison change dans les priorités

| Repère extérieur | Conséquence proposée pour PC Agent |
|---|---|
| [Storage Sense](https://support.microsoft.com/en-us/windows/experience/storage-filemanagement/manage-drive-space-with-storage-sense) et [Microsoft PC Manager](https://pcmanager.microsoft.com/en-us) proposent déjà des parcours de gestion du stockage. | Faire du guidage vers l'action appropriée et expliquer ses effets ; ne pas refaire chaque opération Windows. |
| [WinDirStat](https://windirstat.net/) et [WizTree](https://diskanalyzer.com/faq) rendent les gros postes de stockage explorables. | La lisibilité et la couverture doivent être bonnes avant de faire de la vitesse de scan le principal argument. |
| [System Informer](https://system-informer.com/), [RAMMap](https://learn.microsoft.com/en-us/sysinternals/downloads/rammap) et [HWiNFO](https://www.hwinfo.com/about-software/) donnent déjà beaucoup de détails techniques. | La valeur à tester est l'interprétation adaptée à la question, les mesures pertinentes et la prochaine étape justifiée. |
| [ETW MCP](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/etw-mcp-early-preview-july-2026), en préversion, relie des traces Windows à un assistant IA ; [l'agent des paramètres Windows](https://support.microsoft.com/en-us/windows/experience/exploring-windows-settings) traite des demandes sur les PC compatibles. | « IA + Windows » ne suffit pas à différencier le produit. Tester un parcours plus simple, borné, explicable et utile sans cloud ; considérer les outils experts comme des compléments. |

Trois témoignages motivent des hypothèses à tester : une [saccade intermittente difficile à capturer](https://www.reddit.com/r/techsupport/comments/1gb5xyp), une [RAM dont les totaux semblent incohérents](https://www.reddit.com/r/WindowsHelp/comments/1rh2glj/memory_usage_not_adding_up_which_causes/) et une [perte de fichier attribuée par son auteur au nettoyage de Téléchargements](https://www.reddit.com/r/datarecovery/comments/1ifisy4). Ils appuient respectivement D01, D04/D06 et S01/U03. Leurs causes ne sont pas établies et ces cas ne permettent pas de mesurer la fréquence des besoins ; dates et limites d'accès figurent dans les sources.

## À qui ces améliorations doivent servir

La cible de la spec reste l'utilisateur Windows individuel — développeur, joueur ou utilisateur exigeant — qui veut prendre une décision sans apprendre tous les compteurs Windows.

| Question de l'utilisateur | Résultat utile attendu | Écart principal restant |
|---|---|---|
| « Pourquoi mon PC rame maintenant ? » | Capturer le problème pendant qu'il survient, isoler les ressources concernées, proposer une prochaine vérification. | Les appels successifs produisent surtout des instantanés ; il manque une observation guidée et des diagnostics de fenêtre. |
| « Où passe ma RAM ? » | Comprendre les groupes d'applications et la mémoire non attribuée, sans confondre commit, working set et RAM physique. | Arbre et métriques existent, mais pas encore une explication regroupée et suivie dans le temps. |
| « Trouve-moi 50 Go » | Voir ce qui est récupérable, ce qui est seulement à examiner et pourquoi l'objectif peut être impossible. | La politique sait construire un plan orienté objectif ; le parcours CLI/agent ne l'expose pas complètement. |
| « Dois-je acheter de la RAM ou un SSD ? » | Une décision conditionnelle liée à l'usage à conserver, ou une abstention expliquée. | Inventaire et citations existent ; la répétition des observations, le contexte de charge et le conseil matériel structuré restent à construire. |

<a id="prochain-lot"></a>

## Le prochain lot que je recommande

1. **Maintenir la base intégrée et sa documentation**, en vérifiant les capacités ensemble — Q01 intégré dans `536e382`, validation à conserver pour chaque lot.
2. **Ajouter une capture bornée avec marqueur « ça rame ici »**, réutilisable sans IA — D01.
3. **Calculer des findings sur cette fenêtre et adapter le rapport à la question**, y compris hors ligne — D02, D03, D06.
4. **Relier l'objectif de stockage au plan déjà présent dans `policy`** — S01.
5. **Permettre un arrêt propre et un export expurgé**, puis constituer un corpus rejouable — I01, U01, Q02.
6. **Faire essayer ces parcours sur d'autres PC Windows**, en mesurant aussi l'empreinte de PC Agent — Q03, Q04.

Ces étapes constituent une tranche produit ; les deux premières capacités de capture/diagnostic peuvent être livrées avant le reste. Une GUI minimale peut ensuite rendre ce parcours accessible. MFT, de nouveaux adaptateurs destructifs et les alertes permanentes attendent une preuve de besoin.

<a id="backlog"></a>

## Backlog détaillé

**Priorités de ce document : A = prochain lot utile ; B = suite logique ; C = exploration à valider.** Elles ne remplacent pas les P0/P1/P2 de la spec et ne désignent pas des sévérités de bugs. **Effort : S = localisé ; M = plusieurs composants ; L = chantier important avec validation native.** Ce sont des ordres de grandeur, pas des estimations en jours. Les critères de fin ci-dessous sont proposés ; ils ne sont pas déjà atteints.

### Diagnostic et décisions

| ID / priorité / effort | Amélioration et bénéfice utilisateur | Existant, manque et critère de fin |
|---|---|---|
| **D01 · A · M** | **Observation guidée sur une durée choisie.** L'utilisateur reproduit son ralentissement et marque le moment gênant. | `Collector` et `MetricHistory` existent, mais enregistrent lors des collectes ; pas de commande de capture dédiée. Ajouter une cadence bornée, compteurs de lacunes et marqueur temporel. **Fini quand :** une capture fonctionne sans cloud, restitue une fenêtre contenant le symptôme ou dit qu'il n'a pas été capturé, et s'arrête proprement. La durée d'exemple de 60 s reste un réglage à valider. |
| **D02 · A · M** | **Findings multi-échantillons.** Distinguer pic bref, pression persistante et croissance pendant la charge. | Les diagnostics partent principalement de snapshots ; le comparateur avant/après ne remplace pas l'analyse d'une session. Calculer localement durée observée, dispersion, tendance et temps sous pression, avec lacunes explicites. **Fini quand :** pics, croissance normale et contention soutenue donnent des conclusions différentes dans le corpus ; répéter un même échantillon ne renforce pas la preuve. Une tendance seule ne prouve pas une fuite. |
| **D03 · A · M** | **Mode hors ligne adapté au besoin.** Une question stockage doit conduire à un parcours stockage local. | Dans les états lus, le chemin sans modèle appelle `local_report(&initial)` sans exploiter la question pour choisir les inspections. Proposer des intentions explicites — mémoire, CPU, stockage, observation générale — avant une compréhension libre du langage. **Fini quand :** les parcours principaux fonctionnent sans clé et n'effectuent aucune requête réseau. |
| **D04 · B · M** | **Regroupement par application et résolution locale des noms.** Comprendre « Chrome », « mon IDE », « Docker », plutôt qu'une liste de processus isolés. | Arbre, tri par ressource et alias opaques existent. Il manque un regroupement présenté comme tel et un lien local entre une application nommée par l'utilisateur et ses références. **Fini quand :** les instances sont distinguées, les ambiguïtés demandent un choix local, les noms restent locaux par défaut et la mémoire partagée n'est pas additionnée comme de la RAM exclusive. |
| **D05 · B · M** | **Clarifications utiles et contexte de charge.** Demander ce qui ralentit, depuis quand, sur batterie ou secteur, et quelles applications conserver. | Préférences et question libre existent ; pas de dialogue structuré pour combler une information manquante. Limiter les questions à celles qui changent l'enquête. **Fini quand :** « garde Docker » ou « seulement ce dossier » devient une contrainte locale vérifiable ; une réponse conversationnelle ne vaut jamais approbation d'une action. |
| **D06 · A · M** | **Conclusion compréhensible et prochaine étape concrète.** Répondre à la question avec faits, hypothèses, limites et suite justifiée. | Les citations et trois dimensions du rapport existent sur `main`. Ajouter des conclusions métier locales : RAM non attribuée, stockage insuffisamment couvert, objectif impossible, capture à refaire ; présenter la couverture pertinente pour la question sans masquer la couverture globale. **Fini quand :** le rapport indique ce qui est établi, ce qui manque et quelle observation permettrait de trancher ; les références anciennes sont datées et distinguées des observations actuelles. |
| **D07 · B · M** | **Vérification représentative du résultat.** Savoir si l'action aide réellement l'usage que l'utilisateur veut conserver. | Vérification adaptée au type d'action et comparateur local déjà présents ; les fenêtres restent courtes. Conserver charge/contexte avant et après, attendre une stabilisation et autoriser « non comparable ». **Fini quand :** un changement de RAM n'est pas présenté comme un gain de fluidité ; le critère choisi et le ressenti éventuel restent séparés. |
| **D08 · B · M** | **Conseil matériel conditionnel.** Aider à choisir entre changement d'usage, réglage, maintenance et achat. | Inventaire et observations citables disponibles ; pas de parcours complet U4. Croiser répétition des contraintes et applications à préserver ; distinguer capacité installée, besoin observé et compatibilité inconnue. **Fini quand :** le corpus comporte des cas « ne rien acheter », « mesurer davantage » et « capacité à étudier », sans capacité conseillée déduite du seul commit. |

Points d'appui dans le code : [collecteur](../crates/telemetry/src/lib.rs), [historique](../crates/telemetry/src/history.rs), [diagnostics](../crates/diagnostics/src/lib.rs), [comparateur](../crates/diagnostics/src/compare.rs), [CLI](../apps/cli/src/main.rs). WPR et RAMMap donnent des repères de capture et de sémantique ; ils ne démontrent pas la qualité diagnostique de PC Agent (sources de recherche plus bas).

### Stockage et actions utiles

| ID / priorité / effort | Amélioration et bénéfice utilisateur | Existant, manque et critère de fin |
|---|---|---|
| **S01 · A · M** | **Parcours « récupérer X Go ».** Montrer un plan atteignable ou expliquer ce qui empêche l'objectif. | `Policy::build_plan` sait déjà représenter objectif, récupération immédiate/différée et exclusions. Il manque son exposition complète dans CLI/agent. **Fini quand :** l'utilisateur choisit un périmètre et un objectif ; la réponse sépare espace observé, estimations, choix à examiner et gain immédiat nul de la corbeille ; les données personnelles ne complètent jamais artificiellement le total. |
| **S02 · B · M** | **Navigation et sélection lisibles.** Passer d'un gros dossier à ses sous-dossiers et décider quoi examiner. | Classement fichiers/dossiers et pagination existent ; la CLI est surtout un top N. Ajouter navigation hiérarchique, filtres, sélection/exclusion locale et affichage clair des parties non inspectées. **Fini quand :** l'utilisateur retrouve une cible au-delà de la première page ; la sélection ne double-compte pas parent et enfants. Une treemap est facultative. |
| **S03 · A avant élargissement des actions · M** | **Transmettre les attributs réels jusqu'à la politique.** Préserver la qualité des recommandations sur fichiers cachés, système ou gérés par le cloud. | `Workstation::cleanup_candidates` construit actuellement des `CandidateInput` avec `Attributes::default()`. Ce constat n'établit pas une suppression dangereuse dans le catalogue actuel, qui reste très restreint, mais la classification n'a pas toutes les métadonnées prévues. **Fini quand :** attributs observés/inconnus sont propagés et testés ; l'absence d'une mesure ne devient pas une absence de risque. |
| **S04 · A pour la couverture, B pour enrichir · M** | **Dossiers déplacés et OneDrive compréhensibles.** Expliquer pourquoi le total diffère de Windows et quels éléments ont été ignorés. | Protections et refus de liens/placeholders présents ; `KnownFolders::from_env` repose sur l'environnement, et les fichiers cloud hydratés peuvent être ignorés. S'appuyer sur les Known Folders Windows pour les emplacements réels et valider les cas cloud. **Fini quand :** Documents déplacé, OneDrive en ligne/local et accès refusé donnent une couverture honnête, sans rappel du contenu ni extension de périmètre. [Known Folders](https://learn.microsoft.com/en-us/windows/win32/shell/known-folders), [Files On-Demand](https://support.microsoft.com/en-us/onedrive/save-disk-space-with-onedrive-files-on-demand-for-windows). |
| **S05 · Livré pour le pré-scan · M** | **Coordination des scans autorisés.** Éviter que l'enquête consomme ses tours à attendre. | `diagnose --scan-root` prépare désormais des index bornés avant l'enquête, y compris hors ligne/aperçu. L'agent voit les scans disponibles ; le rapport conserve couverture et limites. Jobs et prolongations restent présents. Restent à améliorer : annulation unifiée I01 et réutilisation entre sessions I03/S07. |
| **S06 · B · L** | **Premier parcours d'action réellement utile sur des données ordinaires.** Aider sans limiter la démonstration à un cache synthétique. | Explorer/Settings sont proposés ; corbeille et fermeture normale ont un backend mais ne sont pas exposées au modèle ; le seul cleanup exécutable est synthétique. Commencer par le guidage vers les outils Windows et une sélection locale étroite pour la corbeille. **Fini quand :** cible, récupération, exclusions, revalidation et bilan sont testés de bout en bout. Un adaptateur de vrai cache doit être validé séparément ; aucun `AppData`/`Temp` générique. |
| **S07 · B · L** | **« Qu'est-ce qui a grossi depuis la dernière fois ? »** Comparer deux scans plutôt que recommencer l'interprétation. | Index en mémoire, sans persistance ni diff utilisateur. Enregistrer périmètre, date, identités et exclusions ; signaler les changements de couverture. **Fini quand :** création, croissance, déplacement, suppression et dossier inaccessible sont distingués ; deux scans non comparables ne produisent pas un faux gain/perte. Dépend d'I03. |
| **S08 · B · M** | **Comprendre les gros postes développeur.** Expliquer WSL/Docker, machines virtuelles, modèles locaux et sorties de build sans les traiter comme déchets. | La politique sait déjà rester prudente sur projets/VM/caches ; manque une explication dédiée. Un VHD WSL contient un système de fichiers, et les nettoyages Docker peuvent concerner des données encore utiles. **Fini quand :** la taille du fichier hôte n'est pas confondue avec l'espace libre invité ; l'app propose une inspection ou un outil officiel sans effacer un VHD, un volume Docker ou un projet. [WSL](https://learn.microsoft.com/en-us/windows/wsl/disk-space), [portée de Docker prune](https://docs.docker.com/reference/cli/docker/system/prune/). |
| **S09 · C · L** | **Backend MFT/USN et index incrémental, si le gain est démontré.** Réduire le délai sur de grands volumes. | Interface `ScanBackend` prévue, backend classique fonctionnel. **Fini quand :** benchmark comparatif publié sur un corpus défini, cohérence avec le backend classique, invalidation du journal gérée, limites d'accès respectées et repli sans élévation. Un classement de vitesse fournisseur ne constitue pas ce benchmark. |

Points d'appui : [stockage](storage.md), [politique](policy.md), [plans orientés objectif](../crates/policy/src/plan.rs), [classification des observations](../crates/agent/src/workstation.rs), [catalogue et limites des actions](actions.md).

### Conduite d'enquête et continuité

| ID / priorité / effort | Amélioration et bénéfice utilisateur | Existant, manque et critère de fin |
|---|---|---|
| **I01 · A · M** | **Progression et interruption de toute l'enquête.** Savoir si l'app mesure, scanne ou attend le fournisseur, puis arrêter sans perdre le bilan. | Traces des outils terminés, annulation des scans et Ctrl+C pendant les actions existent. Le CLI ne propose pas un contrôle unifié de la collecte, de l'attente cloud et du scan. **Fini quand :** interruption = arrêt des nouveaux travaux, clôture des jobs et rapport partiel cohérent ; un appel natif déjà bloqué reste explicitement une limite. |
| **I02 · B · M** | **Questions de suivi dans la même session.** « Et cette application ? », « refais une mesure », « seulement D: ». | Prolongation du même run présente sur `main`, mais pas un vrai parcours de conversation de suivi. **Fini quand :** contexte, périmètre et références sont réutilisés à bon escient ; une mesure trop ancienne est reprise ; élargir un scope reste une décision locale. |
| **I03 · B · L** | **Historique local facultatif et reprise après fermeture.** Retrouver une enquête et comparer des sessions. | Historique système en mémoire et journal d'actions présents ; ils ne sont pas une base de sessions. Ajouter stockage versionné, rétention bornée, purge/export et séparation des boots fondée sur une identité fiable ; une estimation par horloge/uptime ne suffit pas pour corréler durablement les sessions. **Fini quand :** reprise après crash sans secrets ni autorisations persistées ; plans et preuves sont revalidés ; aucune action n'est rejouée. Une base SQLite est une option, pas un objectif produit à elle seule. |
| **I04 · B · M** | **Budget de coût compréhensible avant de continuer.** Choisir entre coût, délai et profondeur d'enquête. | Branche coûts : usage et estimation datée ; `main` : prolongations en tours/outils/temps. Ajouter plafonds configurables, cumul visible et coût estimé de la suite. **Fini quand :** plafond respecté avant un nouvel appel, usage inconnu jamais présenté comme gratuit, et distinction entre estimation client et facturation réelle. Ne pas promettre un plafond monétaire exact sans borne vérifiable côté fournisseur. |
| **I05 · B · M** | **Erreurs cloud récupérables.** Continuer avec les observations locales après panne réseau ou limitation du fournisseur. | Erreurs et rapports partiels existent ; pas de reprise réseau fine dans le transport lu. Distinguer configuration, authentification, indisponibilité et limitation de débit ; reprise bornée des requêtes appropriées. **Fini quand :** le message indique la suite utile, les observations ne sont pas perdues, les actions ne sont jamais rejouées et aucun autre fournisseur n'est choisi implicitement. |

Points d'appui : [agent](../crates/agent/src/lib.rs), [transport Nebius](../crates/agent/src/nebius.rs), [journal](../crates/actions/src/journal.rs). La persistance doit compléter les limites et consentements actuels, pas les contourner.

### Compréhension, confidentialité et distribution

| ID / priorité / effort | Amélioration et bénéfice utilisateur | Existant, manque et critère de fin |
|---|---|---|
| **U01 · A · M** | **Rapport local partageable et expurgé.** Obtenir de l'aide sans publier son inventaire ou ses chemins. | JSON local et enveloppe cloud minimisée existent, avec des usages différents. Ajouter un export dédié support en Markdown/HTML/JSON, aperçu et choix des éléments partagés. **Fini quand :** noms de profil, chemins, secrets, question libre et métadonnées sensibles sont contrôlés ; exporter ne transmet rien ; version du logiciel, limites et preuves utiles restent lisibles. Un JSON local brut n'est pas automatiquement partageable. |
| **U02 · A · M** | **Premier lancement et `doctor` complets.** Comprendre ce qui fonctionnera avant de lancer une longue enquête. | `doctor` contrôle plateforme/configuration, mais annonce lui-même ses limites Windows/storage/base. Ajouter versions et capacités réellement détectées, raisons d'indisponibilité et explications simples pour choisir les scopes. **Fini quand :** utilisateur sans Rust ni clé réussit un premier diagnostic local et sait pourquoi une mesure manque, sans invitation générale à lancer en administrateur. |
| **U03 · B · M** | **Réglages locaux utilisables.** Protéger une application ou un dossier sans écrire du JSON à la main. | Préférences versionnées, exclusions et applications préservées existent. Ajouter commandes guidées puis UI, validation et aperçu de l'effet. **Fini quand :** une préférence persiste, peut être retirée et est appliquée aux recommandations comme aux actions ; les permissions restent distinctes. |
| **U04 · B · M** | **Gestion des clés dans le coffre Windows.** Configurer, remplacer et supprimer sa clé sans manipuler de fichier en clair. | Variables d'environnement et fichier du profil pris en charge ; aucun coffre implémenté. **Fini quand :** stockage et suppression passent par le coffre, provenance affichée, erreurs explicites, aucun secret dans exports/journaux. L'API [CredWriteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew) constitue une piste d'implémentation à valider. |
| **U05 · B · M** | **Voir et maîtriser ce qui part au cloud.** Conserver l'opt-in tout en rendant la minimisation compréhensible. | `--preview-cloud` montre la première charge, pas toute l'enquête ; la question est transmise telle quelle. Ajouter aperçu local des catégories envoyées, journal local borné des échanges expurgés et possibilité de reformuler une question sensible. **Fini quand :** aucun export ou historique complet n'est envoyé implicitement et les envois ultérieurs respectent le contrat annoncé. Ne pas promettre l'anonymat. |
| **U06 · B · L** | **Interface Tauri minimale, accessible et en français/anglais.** Rendre les parcours utilisables sans maîtriser les options CLI. | CLI officiel et Core Rust réutilisable déjà présents. Commencer par question guidée, capture, preuves, choix d'action et historique ; conserver un mode simple et les détails à la demande. **Fini quand :** mêmes décisions et garde-fous que la CLI, navigation clavier/lecteur d'écran, statuts compréhensibles sans couleur et aucun terminal helper intempestif. Traductions par identifiants/templates, pas par duplication des règles métier. |
| **U07 · A pour version testable, B pour distribution durable · M/L** | **Un build facile à essayer et identifiable.** Éviter d'exiger un environnement de développement. | Artefact CI Windows disponible, sans installateur de production. Fournir une version repérable, instructions courtes, empreinte du binaire et procédure de retrait des données locales ; ensuite étudier signature, installateur et mises à jour vérifiées. **Fini quand :** un testeur sur PC distinct installe/lance/désinstalle et sait quel commit il utilise. Choix de licence et compatibilité des dépendances à trancher avant publication, sans intégrer ici du code concurrent. |

### Qualité diagnostique et codebase durable

| ID / priorité / effort | Amélioration et bénéfice utilisateur | Existant, manque et critère de fin |
|---|---|---|
| **Q01 · Intégré · M** | **Intégrer les évolutions sans régression de contrat.** Conserver les acquis des deux branches. | La divergence initiale a été résolue dans `main@536e382`, base de ce lot. Prolongations/citations/statuts, contexte compact, coût et NVML sont réunis ; la validation du contexte stockage porte sur l'ensemble du workspace. Les recettes produit sur d'autres PC restent Q02/Q03. |
| **Q02 · A · M** | **Corpus de diagnostics et replay.** Mesurer si une nouvelle version répond plus justement. | Beaucoup de tests unitaires et réponses modèle scriptées, mais pas un benchmark produit représentatif. Construire cas normaux, pics, pression, ambiguïtés, capteurs absents, contraintes utilisateur et stockage protégé ; replay local déterministe, puis comparaison séparée des modèles autorisés. **Fini quand :** faits erronés, conclusions trop fortes, abstentions utiles, actions bloquées, délai et coût sont suivis ; un joli texte ne suffit pas à gagner le benchmark. |
| **Q03 · A · M** | **Recette sur plusieurs configurations réelles.** Réduire les mauvaises surprises hors du PC de développement. | Tests natifs/CI et essais live documentés, mais la checklist sur PC Windows distinct reste ouverte. Couvrir au minimum machine sans NVIDIA, portable hybride, autre stockage/profil et compte non administrateur ; corpus jetable pour les effets. **Fini quand :** matrice capacités attendues/observées et erreurs connues publiée pour le commit livré ; les cas non essayés restent identifiés. |
| **Q04 · A · M** | **Empreinte propre mesurée.** PC Agent ne doit pas aggraver le symptôme qu'il observe. | Budgets coopératifs présents ; objectifs CPU/RAM/disque non démontrés sur un corpus représentatif. Mesurer l'ensemble app/helpers, collecte, scan et coût du réveil GPU ; adapter la cadence sur batterie. **Fini quand :** consommation et temps jusqu'au premier résultat sont publiés avec machine/charge/périmètre ; le processus de PC Agent reste visible dans les consommateurs. |
| **Q05 · B · M** | **Registre des capacités partagé.** Éviter les contradictions entre `doctor`, outils, rapport et README. | Schémas d'outils centralisés et statuts présents, mais inventaire des capacités réparti. **Fini quand :** une capacité expose support, permissions, coût de collecte et raison d'indisponibilité aux différentes interfaces ; les docs distinguent implémenté, exposé et validé. Le GPU NVML disponible avec inventaire GPU encore incomplet est un cas à couvrir. |
| **Q06 · B, au fil des changements · M** | **Refactors ciblés autour des responsabilités.** Rendre les prochaines évolutions locales et testables. | Sur `main`, budget, rapport et tests sont déjà séparés ; la branche ajoute coût et payload compact. Préserver ces limites, isoler futures sessions/capture, garder orchestration dans l'agent et règles dans diagnostics/policy. **Fini quand :** ajouter une métrique ou un parcours ne duplique pas validation, consentement ou présentation ; interfaces courtes et scénarios comportementaux restent lisibles. Aucune réécriture générale ni framework de plugins sans besoin établi. |
| **Q07 · B, avant GUI/persistance · M** | **Contrats versionnés et compatibles.** Préserver les scripts, exports et futures sessions. | Versions JSON présentes ; nouveaux champs/type IPC devront évoluer. Définir compatibilité, migrations et représentation exacte des octets pour JavaScript. **Fini quand :** vieux exports se lisent ou échouent clairement, entiers 64 bits ne perdent pas de précision, la CLI garde un document JSON unique et une sémantique documentée des codes de sortie. |
| **Q08 · B · M** | **Vérifications ciblées des frontières sensibles.** Empêcher qu'une évolution casse les garanties existantes. | CI Windows/Linux, tests de plans/chemins/collecteurs et dépendances verrouillées présents. Ajouter tests génératifs/fuzzing sur parseurs, chemins, journal et validation d'outils ; suivi des dépendances/licences ; tests d'expurgation avec données hostiles. **Fini quand :** corpus reproductible de régressions, contrôles utiles en CI et aucun test d'effet destructif sur des données utilisateur. Éviter les tests qui ne font que recopier l'implémentation. |

### Extensions cohérentes, après les parcours principaux

| ID / priorité / effort | Piste et intérêt | Condition avant développement ou livraison |
|---|---|---|
| **E01 · B · L** | **Attribution disque/GPU plus précise.** Répondre « quel disque, quel moteur, quelle activité ? » plutôt que seulement relever une saturation globale. | Les compteurs agrégés et NVML sont un départ, pas une attribution causale. Définir identités, sources et couverture par appareil/processus ; ne pas annoncer des E/S physiques d'après de simples compteurs d'E/S de processus. Prioriser les cas effectivement bloquants du corpus. |
| **E02 · C · L** | **Capteurs matériels complémentaires.** CPU thermique/fréquence, autres GPU, stockage et matériel hybride. | NVIDIA/NVML existe déjà sur la branche. Pour chaque nouveau capteur : bénéfice sur une décision, disponibilité, privilèges, coût et tests sur matériel réel. LibreHardwareMonitor est une référence à étudier ; ne pas embarquer un driver ou une dépendance native avant validation technique et de distribution. Un indicateur SMART isolé ne doit pas devenir une prédiction de panne. |
| **E03 · C · M** | **Batterie, chauffe et veille.** Parcours utile aux portables, avec état secteur/batterie et comparaison de sessions. | Commencer par importer localement des rapports de diagnostic existants et minimisés. [Powercfg](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/powercfg-command-line-options) et [SleepStudy](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/modern-standby-sleepstudy) documentent ces usages ; SleepStudy dépend de Modern Standby et son exécution documentée demande des droits administrateur. Ne pas ajouter de modification automatique de plan d'alimentation ni d'élévation générale. |
| **E04 · C · M/L** | **Chronologie des changements et événements.** Aider pour « c'est lent depuis hier », démarrage ou plantages récurrents. | Sélection bornée d'événements et versions réellement observés, collecte locale explicite, comparaison temporelle. Lister des erreurs Windows ou une mise à jour proche du symptôme ne prouve pas sa cause. Séparer historique indisponible et absence d'événement. |
| **E05 · C · L** | **Passage guidé vers WPR/WPA et traces spécialisées.** Escalader une enquête qui atteint la limite des compteurs simples. | Profils connus, collecte bornée, privilèges explicites, analyse locale ou export contrôlé, retour compréhensible. Examiner aussi ETW MCP comme complément existant. Ne pas transformer le modèle en shell ni envoyer des ETL bruts au cloud par défaut. |
| **E06 · C · M/L** | **Mesure de fluidité en jeu ou d'une tâche précise.** Vérifier les saccades/FPS/temps d'exécution au lieu de les déduire de la RAM. | Parcours explicitement opt-in, source et fenêtre visibles, charge comparable. [PresentMon](https://github.com/GameTechDev/PresentMon) fournit un projet de référence pour les performances graphiques Windows ; intégration et bénéfice restent à évaluer. Aucun gain de FPS extrapolé d'un compteur système. |
| **E07 · C · M/L** | **Triage réseau.** Distinguer PC lent, application distante lente et connexion problématique. | Besoin à confirmer avec les testeurs. D'abord indiquer la couverture manquante ; ensuite lectures locales et tests actifs bornés vers une destination autorisée. Pas de capture de contenu réseau ni de test Internet implicite. |
| **E08 · C · M** | **Recherche documentaire avec sources.** Expliquer un problème identifié à partir de documentation applicable. | Tavily n'est pas encore intégré. N'envoyer que le composant/version nécessaires avec permission distincte ; préférer source officielle datée ; traiter les pages comme des données non fiables. La recherche complète les mesures et ne permet ni commande libre ni attribution causale par simple ressemblance. |
| **E09 · C · L** | **Alertes proactives et références personnelles.** Prévenir un problème récurrent sans surveillance intrusive. | Dépend de D01/D02, I03, Q02/Q04 et d'une demande utilisateur confirmée. Démarrage et collecte permanente opt-in, seuils contextualisés, temporisation et budget ; mesurer les faux positifs avant d'activer des notifications. Jamais d'action automatique pour justifier une alerte. |

<a id="livraison"></a>

## Ordre de livraison et validation produit

| Lot | Livrable démontrable | Dépendances et sortie attendue |
|---|---|---|
| **0 — Une base cohérente** | Un commit réunissant les acquis des branches, documentation alignée et artefact testable. | Q01, début Q05/U07. Les correctifs déjà faits ne réapparaissent pas dans le backlog comme manquants. |
| **1 — Observer une vraie lenteur** | Capture hors ligne → marqueur → conclusion sur une fenêtre → prochaine mesure ou abstention. | D01–D03, D06, I01 ; corpus Q02 dès le début. Premier parcours testable avant GUI complète. |
| **2 — Prendre une décision stockage** | Objectif choisi → périmètre autorisé → candidats et exclusions → résultat atteignable ou non. | S01–S04 ; conserver les actions actuelles, puis S06 après recette dédiée. Pas de dépendance obligatoire à MFT. |
| **3 — Faire vérifier le produit** | Export expurgé, test sur autres PC, comparaison de versions et empreinte mesurée. | U01/U02/U07, Q02–Q04. Certaines validations commencent dès le lot 1 ; ce lot en rassemble les preuves. |
| **4 — Revenir et approfondir** | Questions de suivi, sessions opt-in, comparaison de scans, budget de coût et interface minimale. | I02–I05, S07, U03–U06, Q06/Q07. Revalidations et consentements restent locaux. |
| **5 — Spécialiser à la demande** | Choisir une extension E01–E09 ou S09 à partir d'un cas utilisateur non résolu. | Pas de lot « ajouter tout ». Chaque extension doit améliorer une décision mesurable du corpus. |

Pour la première étude d'usage, proposer un petit panel volontaire de développeurs, joueurs et utilisateurs de portables sur leurs tâches habituelles. Le nombre de participants est à décider ; aucun entretien n'a encore été mené dans cette revue. Observer : peuvent-ils lancer l'enquête, comprendre la différence entre fait/hypothèse/limite, protéger ce qu'ils souhaitent conserver et choisir la suite sans aide ?

Suivre en priorité **le temps jusqu'à une décision utile**, les affirmations non étayées, les abstentions pertinentes, les erreurs de manipulation, l'effet réellement mesuré et le coût total de l'enquête. Comparer règles seules et agent avec outils sur les mêmes scénarios. Les objectifs chiffrés de CPU/RAM de la spec restent des objectifs jusqu'au benchmark ; ni étoiles GitHub, ni votes Reddit, ni qualité du texte ne valident le produit.

<a id="limites-produit"></a>

## Limites à conserver et fausses bonnes idées

**Wanted limitations durables :** cloud et historique facultatifs ; consentement local lié à une action précise ; absence de commandes arbitraires pilotées par le modèle ; lecture de métadonnées pour le scan ; provenance des chiffres ; inconnus explicites ; refus de conclure si les preuves manquent. Aucune amélioration ci-dessus ne doit supprimer ces propriétés.

**Wanted limitations du prototype, réexaminables :** Windows uniquement ; catalogue d'actions étroit ; nettoyage permanent limité au cache synthétique ; fournisseur/modèle du parcours actuel. Les élargir demande un besoin et une validation séparés. « Wanted » ne signifie pas « à garder identique pour toujours » : notamment, le cache synthétique est une frontière de livraison actuelle, pas le résultat produit final.

**Covered limitations à améliorer sans les masquer :** échantillons trop courts, couverture partielle, capteurs indisponibles, dates/scans non immuables, budgets natifs coopératifs, absence de persistance, incertitude sur le gain récupérable et la causalité. Les reconnaître honnêtement ne signifie pas que la fonction manquante est implémentée.

À ne pas prioriser : booster de RAM, nettoyeur de registre, nettoyage universel d'AppData, suppression de données personnelles pour atteindre un objectif, désactivation automatique de protections, gestion de pilotes/BIOS/overclocking, score unique de santé, achats matériels automatiques, shell administrateur IA, agent de parc distant et support multiplateforme natif. Ces pistes diluent le produit ou contredisent ses garanties. Un mode de modèle local pourrait être étudié plus tard si les testeurs le demandent ; le mode hors ligne utile vient d'abord.

Les problèmes de confiance rapportés sur les nettoyeurs justifient de tester la compréhension des permissions et des effets ; ils ne prouvent pas que tous ces outils sont dangereux. De même, une fonctionnalité concurrente est un repère, pas une obligation de parité.

## Suivi de la revue technique initiale

Les sections suivantes conservent les corrections et limites de la revue de `pc-agent-review.md`. **Le statut « corrigé » de cet historique renvoie au socle et à `main@92b7a28`**, pas à la validation d'une fusion avec la branche NVML. Les recommandations nouvelles sont celles du backlog ci-dessus.

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
| 15. Documentation en retard | **Corrigé** : README, architecture, actions, stockage, télémétrie et validation alignés avec l’implémentation. Les branches sont intégrées dans `536e382` ; le registre de capacités partagé avec `doctor` reste **à développer** (Q05). |
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

- Le modèle choisit uniquement des outils déclarés, des findings et des observations validés localement. Il ne
  fournit ni nombres inventés, ni commandes, ni autorisation d’exécution.
- Une action exige une validation locale explicite. `--json` et une entrée non interactive
  n’exécutent jamais les plans. Le cloud exige un opt-in et respecte les préférences utilisateur.
- Le seul nettoyage permanent est celui du cache synthétique marqué de pcagent : fichiers directs,
  manifeste borné, NTFS et une seule liaison par fichier. Aucun nettoyage générique d’AppData,
  de dossiers personnels ou système ; aucune élévation ni fermeture forcée.
- La télémétrie et les effets natifs ciblent Windows. Ailleurs, ils annoncent `unsupported`.
- Les scans lisent les métadonnées. La comparaison de contenu exige l'option locale séparée `--compare-content` ; liens et placeholders reconnus restent exclus, sans envoi du contenu ni des empreintes.
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

Validation historique du lot `92b7a28` avec Rust 1.90.0 : **214 tests réussis sous Windows**, **203 sous
Linux (WSL)** ; le test live Nebius est ignoré sur les deux plateformes. `cargo fmt --all --
--check` et `cargo clippy --workspace --all-targets --locked -- -D warnings` passent sur les deux
plateformes. Le binaire Windows compile avec `cargo build --release --locked -p pcagent`.

<a id="sources-recherche"></a>

## Recherche produit — notes de sources (27 septembre 2026)

Recherche menée à partir de la vision et des parcours U1–U5 de `SPEC_PC_AGENT.md`, du README et
de ce suivi. Les observations ci-dessous décrivent les sources ; les propositions qui en découlent
sont des choix de produit à valider, pas des capacités déjà implémentées ni des bugs démontrés.
L'échantillon couvre des outils comparables et complémentaires, sans prétendre représenter tout
le marché.

**Base de comparaison.** L'extension de budget, les citations d'observations et les trois dimensions
exécution/couverture/solidité existent sur `main` à `92b7a28`. NVML, les coûts/tokens et la réduction
du contexte cloud existent sur la branche `nvml-cost-tokens` observée à `0528675`. Cette recherche
ne les remet pas dans la liste des fonctions absentes ; leur intégration commune doit être
vérifiée séparément. Un README plus ancien sur une branche ne suffit pas à établir un manque.

### Sources et conséquences possibles

Toutes les sources ont été consultées le **27 septembre 2026**. « Sans date affichée » signifie que
la page consultée ne fournit pas de date éditoriale exploitable ; la date de consultation n'est
pas sa date de publication. Les capacités sont celles décrites par leurs éditeurs, sans benchmark
comparatif exécuté. Les témoignages Reddit ne servent pas de références techniques : ce sont
des cas individuels, sans représentativité ni cause vérifiée. Les billets RP12 et RP14 ont été
lus dans le contenu indexé du moteur ; leur ouverture directe n'a pas abouti dans cet environnement.

| Réf. et source | Observation établie par la source | Déduction pour PC Agent et limites |
|---|---|---|
| **RP01 — [Microsoft PC Manager](https://pcmanager.microsoft.com/en-us)**, page officielle, sans date affichée | Microsoft présente le nettoyage, la gestion des gros fichiers, les raccourcis Windows et une fonction de « boost ». | Ces parcours simples existent déjà. PC Agent doit faire gagner une décision expliquée : ce qui a été observé, ce qui reste incertain et comment vérifier un changement. La page commerciale ne prouve pas de gains de performances. |
| **RP02 — [Storage Sense](https://support.microsoft.com/en-us/windows/experience/storage-filemanagement/manage-drive-space-with-storage-sense)**, Microsoft Support, sans date affichée | La gestion des temporaires, de la corbeille, des Téléchargements et des contenus cloud comporte des réglages distincts. Storage Sense intervient sur le lecteur système ; rendre un fichier cloud disponible uniquement en ligne n'est pas le supprimer définitivement. | Expliquer les effets et le périmètre d'une proposition, puis orienter vers l'outil Windows adapté lorsque c'est suffisant. Ne pas confondre suppression, corbeille et retrait de la copie locale. Ne pas présenter PC Agent comme nécessaire pour tout nettoyage standard. |
| **RP03 — [RAMMap](https://learn.microsoft.com/en-us/sysinternals/downloads/rammap)**, Sysinternals, publication du **26 mars 2026** | RAMMap expose plusieurs vues de la mémoire physique, dont les catégories d'usage, les working sets, le cache de fichiers et les listes standby ; il sauvegarde et recharge des snapshots. | Ajouter une lecture pédagogique de la mémoire attribuée et non attribuée, des comparaisons à des instants identifiés et un relais vers RAMMap pour les cas approfondis. Une somme de processus ne doit pas devenir un faux total de toute la mémoire ; un pourcentage élevé n'explique pas à lui seul une lenteur. |
| **RP04 — [System Informer](https://system-informer.com/)**, site officiel, sans date affichée | Les graphes relient un point historique au processus concerné, même après sa fin. L'outil présente aussi l'activité disque, réseau et GPU. | Conserver l'identité et le contexte des processus disparus pendant une session : un pic ne doit pas devenir inexplicable parce que le programme s'est terminé. Une attribution temporelle reste distincte d'une preuve de causalité. Ne pas reproduire tout son catalogue d'opérations sur les processus. |
| **RP05 — [FAQ WizTree](https://diskanalyzer.com/faq)**, éditeur, sans date affichée | La FAQ distingue taille et allocation, explique qu'une partie des métadonnées NTFS n'apparaît pas comme fichiers et précise que son scan NTFS rapide exige des droits administrateur. | Montrer la couverture et les écarts au volume ; développer l'allocation réelle sans promettre que la somme inventoriée égale exactement l'espace utilisé. MFT reste une optimisation conditionnelle à mesurer, avec parcours normal conservé. La vitesse commerciale d'un concurrent n'est pas un benchmark de PC Agent. |
| **RP06 — [WinDirStat](https://windirstat.net/)**, projet officiel open source, sans date affichée | Le projet décrit listes triables, gros fichiers, extensions, treemap, comparaison des tailles logiques/physiques et sauvegarde/export de scans. Sa page actuelle mentionne également le scan NTFS direct lorsque disponible. | Prioriser la navigation et la compréhension des résultats, puis la comparaison de scans. Ne pas fonder un choix MFT sur l'ancienne image d'un WinDirStat uniquement lent. PC Agent dispose désormais d'une comparaison de contenu locale, facultative et bornée ; la recherche exhaustive de doublons reste distincte. |
| **RP07 — [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor)**, README officiel, branche évolutive consultée à cette date | Le projet fournit application et bibliothèque de capteurs pour plusieurs familles CPU/GPU et stockages ; certaines lectures demandent les droits administrateur. Il signale aussi des différences entre cartes mères. | Étudier un adaptateur optionnel pour combler un besoin démontré, avec matrice de matériels et mesure de surcharge. L'existence d'une bibliothèque ne garantit pas la couverture sur chaque PC. Ne pas imposer de pilote ou d'élévation permanente pour rendre un état de couverture plus flatteur. |
| **RP08 — [HWiNFO](https://www.hwinfo.com/about-software/)**, présentation officielle, sans date affichée | HWiNFO présente inventaire détaillé, suivi de capteurs, journaux, graphes et exports XML/CSV/HTML. | Un export lisible, une timeline et un résumé de session sont des attentes déjà satisfaites par des outils techniques. PC Agent peut apporter la sélection des preuves utiles et une prochaine étape compréhensible. Les formulations commerciales de prédiction de panne ne constituent pas une validation à reprendre. |
| **RP09 — [Introduction à Windows Performance Recorder](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/introduction-to-wpr)**, Microsoft Learn, mise à jour du **21 avril 2022** | WPR propose des profils de traces et distingue enregistrement sur fichier et buffers circulaires en mémoire pour les événements imprévisibles. Les niveaux légers et détaillés ont des coûts différents. | Introduire d'abord une observation légère, bornée et consentie, avec marqueur « le problème vient d'arriver ». Une capture ETW ciblée est une étape avancée possible lorsque les compteurs ordinaires ne suffisent pas. Ni capture permanente par défaut, ni promesse que tout incident sera conservé dans un buffer borné. |
| **RP10 — [Process Monitor](https://learn.microsoft.com/en-us/sysinternals/downloads/procmon)**, Sysinternals, publication du **10 septembre 2026** | Procmon enregistre activité fichiers/registre/processus, conserve les traces et permet des filtres non destructifs. Les données comprennent notamment chemins d'exécutables, lignes de commande, utilisateurs et sessions. | Prévoir un paquet de diagnostic local partageable avec aperçu et expurgation, plutôt qu'un envoi automatique de traces brutes. Des filtres d'affichage ne sont pas une garantie que les données sensibles ont disparu du fichier enregistré. |
| **RP11 — [PresentMon](https://github.com/GameTechDev/PresentMon)**, README officiel, branche évolutive consultée à cette date | Le projet capture des durées/latences CPU, GPU et affichage par image, propose un export CSV et documente des restrictions d'accès ainsi que des limites selon l'API graphique et l'ordonnancement GPU. | Pour le public joueur, les temps d'image apporteraient une mesure du symptôme plus directe qu'un pourcentage GPU moyen. Piste spécialisée ultérieure, avec validation par configuration ; les capteurs NVML déjà développés ne remplacent pas cette mesure et ne sont pas à redévelopper. |
| **RP12 — [« How to troubleshoot intermittent video & audio glitch »](https://www.reddit.com/r/techsupport/comments/1gb5xyp)**, r/techsupport, **24 octobre 2024** | Une personne décrit une saccade audio/vidéo rare, pouvant ne pas réapparaître pendant une semaine, et demande comment journaliser pour l'observer. | Besoin plausible d'un parcours « reproduire et marquer le symptôme », de continuité de session et de rétention explicite. Ce témoignage ne démontre ni la cause ni la fréquence du problème dans la population. |
| **RP13 — [« Memory usage not adding up which causes performance issues »](https://www.reddit.com/r/WindowsHelp/comments/1rh2glj/memory_usage_not_adding_up_which_causes/)**, r/WindowsHelp, **28 février 2026** | La personne compare environ 80 % de mémoire au total des applications et ne comprend pas l'écart. La discussion dérive vers des désinstallations et des scripts de modification de Windows. | Ajouter « ce chiffre mesure quoi ? », la part non ventilée et une distinction entre observation, hypothèse et intervention. L'impression de RAM élevée ne doit pas déclencher des changements génériques. Aucun conseil technique des commentaires n'est adopté comme vérité. |
| **RP14 — [« Single important .txt file got auto deleted from Windows 11 Storage Sense »](https://www.reddit.com/r/datarecovery/comments/1ifisy4)**, r/datarecovery, **1er février 2025** | Une personne rapporte la perte d'une sauvegarde conservée dans Téléchargements, qu'elle attribue à Storage Sense. Sa configuration et la cause ne sont pas vérifiées. | L'emplacement et l'âge d'un fichier ne prouvent pas qu'il est inutile. Conserver des exclusions simples et une liste exacte à examiner ; éviter le vocabulaire « déchets » pour des fichiers personnels. Ce billet ne prouve pas un bug de Storage Sense ni son comportement par défaut actuel. |
| **RP15 — [Agent dans les paramètres Windows](https://support.microsoft.com/en-us/windows/experience/exploring-windows-settings)**, Microsoft Support, sans date affichée | Sur les Copilot+ PC pris en charge, un modèle local associe les demandes aux paramètres. Les changements demandent une action de l'utilisateur et peuvent être annulés ; une recherche ordinaire sert de repli. | « Comprendre une phrase et ouvrir/modifier un réglage » est déjà une capacité concurrente. PC Agent doit se distinguer par l'investigation mesurée et les résultats avant/après, avec un mode local utile et des contrôles simples. Ne pas annoncer que ces fonctions Windows existent sur tous les PC. |
| **RP16 — [ETW MCP Early Preview](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/etw-mcp-early-preview-july-2026)**, Microsoft Learn, mise à jour du **31 juillet 2026** | Microsoft documente un serveur MCP local sans interface graphique pour interroger des traces ETL avec un assistant IA, filtrer par processus/période et comparer des traces. C'est une préversion ; la documentation demande de vérifier les conclusions contre les traces. | Concurrent et complément à surveiller : l'association « IA + vraie télémétrie + preuves » n'est pas une nouveauté exclusive de PC Agent. Une voie d'import ou de relais peut servir les cas experts ; elle ne doit pas devenir une dépendance obligatoire du parcours simple ni une validation automatique des raisonnements du modèle. |
