# PC Agent — feuille de route produit, améliorations et limites

**Réévaluation : 27 septembre 2026. Base examinée : [main@e8e6c38](https://github.com/nassim-arifette/pc-agent/commit/e8e6c382a228432db57ae0fd7ff7b2b138062eff), après les PR #14 et #15.**

PC Agent doit permettre de **comprendre un ralentissement pendant qu'il se produit** et de **voir rapidement ce qui occupe le PC pour décider quoi conserver, déplacer ou nettoyer**. La vitesse d'inventaire, la qualité des explications et le contrôle des actions sont trois exigences du même produit.

Cette feuille de route rassemble les améliorations restantes, leur priorité, leurs dépendances et leurs critères de fin. Elle distingue les fonctions déjà livrées, les limites explicitement couvertes et les restrictions volontaires. Les recommandations ne sont ni des fonctions implémentées ni une promesse de calendrier.

Lecture : [état actuel](#etat-du-code), [priorités réévaluées](#priorites), [MFT et USN](#mft-usn), [backlog complet](#backlog), [ordre de livraison](#livraison), [limites](#limites-produit), [revue historique](#revue-initiale), [sources](#sources-recherche).

<a id="etat-du-code"></a>

## 1. État actuel pris en compte

La base de référence est désormais **un main intégré**. La prolongation d'enquête, les citations, les statuts, NVML, les coûts et le contexte des fichiers sont réunis. La divergence entre branches décrite dans la précédente version est résolue. Les travaux non fusionnés d'autres branches ne sont pas comptés comme livrés.

| Domaine | Déjà implémenté ou exposé sur cette base | Ce qui manque encore |
|---|---|---|
| Diagnostic | Collectes Windows CPU, mémoire, processus, E/S et compteurs disque/GPU ; historique système en mémoire ; findings locaux avec preuves et limites ; capteurs NVIDIA/NVML lorsqu'ils sont disponibles. | Capture guidée continue, interprétation d'une fenêtre représentative, regroupement par application, attribution disque/GPU plus fine et validation sur plusieurs configurations. |
| Enquête | Outils déclarés, références opaques, observations conservées ; citations matériel/stockage/historique ; prolongation interactive consentie du même run ; consommation de tokens et estimation datée du coût. | Conversation de suivi, reprise après fermeture, plafonds utilisateur de coût, annulation unifiée et récupération réseau plus fine. |
| Rapport | Séparation `execution_status`, `coverage` et `diagnostic_support` ; faits reconstruits localement ; anciennes observations distinguées ; JSON unique. | Conclusions réellement adaptées au besoin, notamment hors ligne, couverture pertinente pour la question et prochaine étape guidée. Une exécution réussie ne prouve pas que le PC est sain. |
| Stockage | Scanner classique progressif et borné, tailles logiques/allouées, liens physiques dédupliqués, refus de liens/placeholders, pagination et index en mémoire ; pré-scan des `--scan-root`. | Inventaire rapide NTFS/MFT, index persistant actualisé par USN, navigation utilisateur, objectif « récupérer X Go » exposé de bout en bout et comparaison de scans. |
| Identité et contexte des fichiers | Alias distincts de fichiers et dossiers par scan, liens parent/enfant et restitution des vrais chemins dans la CLI et le JSON local ; rôles/application suggérés par règles locales ; récupération décrite conditionnellement. | Identités durables de l'index, association à une installation réellement vérifiée, récupération testée et propagation complète des attributs à la politique. |
| Contenu et confidentialité | `--compare-content` local, facultatif et borné ; groupes de même contenu par SHA-256 ; `--share-file-names` facultatif avec liste finie et aperçu ; contenu et empreintes restent locaux. | Recherche étendue de doublons, comparaison entre scopes/dossiers, sélection manuelle des noms partagés, diagnostic antivirus et vérification de formats corrompus. |
| Actions | Propositions Explorer/Settings/cache synthétique, manifeste et approbation locale, revalidation, journal, bilan partiel et vérification adaptée à l'effet. | Parcours utile sur données ordinaires. La corbeille et la fermeture normale ont un backend, mais ne sont pas proposées par le modèle ; la suppression permanente exécutable reste limitée au cache synthétique. |
| Interface et distribution | CLI, `doctor`, modes hors ligne/aperçu, préférences versionnées, artefact Windows en CI ; helpers de tests sans fenêtre de terminal. | Interface Tauri utilisable, réglages guidés, export support expurgé, coffre Windows pour la clé, installation/distribution durable. |
| Qualité | Tests Windows/Linux, CI sans modification du commit évalué, contrats et protections ciblés, essais modèle documentés sur le PC de développement. | Corpus produit rejouable, benchmark de grands volumes, recettes sur d'autres PC et mesure de l'empreinte totale. |

**Portée de cette réévaluation.** Lecture du document existant, de la [spec](../SPEC_PC_AGENT.md), des contrats et points d'intégration du CLI, de l'agent, des diagnostics, du stockage, de la politique et des actions ; vérification des sources officielles pertinentes. Aucun nouveau benchmark concurrent, entretien utilisateur, scan du PC ou appel live au fournisseur n'a été exécuté pour rédiger ce document. Les validations existantes restent attachées à leur commit ; une CI verte ne valide pas tous les usages réels.

Repères : [README](../README.md), [stockage](storage.md), [télémétrie](telemetry.md), [actions](actions.md), [validation](validation.md), [comparaison des modèles](model-comparison.md).

<a id="priorites"></a>

## 2. Comment les priorités sont décidées

La cible reste l'utilisateur Windows individuel : développeur, joueur ou utilisateur qui veut comprendre son PC sans apprendre tous les compteurs Windows. Le produit doit être utile sans clé API, puis permettre à l'agent d'approfondir à partir des mêmes observations.

| Besoin | Résultat attendu | Conséquence sur la priorité |
|---|---|---|
| « Qu'est-ce qui remplit mon disque ? » | Un inventaire rapidement exploitable, une arborescence, les vrais noms localement, des tailles justes et des exclusions visibles. | MFT, index local, navigation et comptabilité font partie du cœur stockage. |
| « Relance l'analyse » ou « et ce dossier ? » | Retrouver les résultats et les actualiser sans rescanner systématiquement le volume. | USN et persistance de l'index suivent immédiatement l'inventaire initial ; ils n'attendent pas l'historique complet des conversations. |
| « Pourquoi ça rame ? » | Observer le symptôme, montrer les ressources concernées et ce que les données permettent réellement de conclure. | Capture, findings sur une durée et conclusion guidée restent prioritaires. |
| « Trouve-moi 50 Go » | Un plan explicable, avec ce qui est récupérable maintenant, plus tard ou seulement à examiner. | Le parcours d'objectif et une première action réelle contrôlée doivent aboutir ; une démo de cache synthétique ne suffit pas au produit final. |
| « Puis-je supprimer ça ? » | Comprendre le rôle, les copies, les dépendances et les possibilités de récupération du fichier précis. | Contexte local, doublons confirmés et récupération sont des chantiers distincts, liés aux actions. |
| « Est-ce infecté ou corrompu ? » | Une vérification adaptée, avec son périmètre et ses résultats, ou un relais vers l'outil compétent. | Ajouter un parcours de triage spécialisé ; le nom, la taille et l'âge ne permettent pas de répondre. |
| « Dois-je acheter du matériel ? » | Un avis conditionnel fondé sur l'usage à conserver et plusieurs observations. | Le conseil matériel vient après la qualité de mesure et la vérification du résultat. |

**A — socle du prochain produit utilisable :** essentiel à l'un des deux parcours principaux ou prérequis de sa livraison. **B — suite directe :** rend ces parcours plus complets et réutilisables. **C — spécialisation :** utile à certains cas, à sélectionner selon les besoins observés. **Transversal :** travail à faire dans chaque lot concerné.

Ces lettres sont des priorités produit, pas des sévérités de bugs. Les P0/P1/P2 de la spec décrivent le découpage initial du hackathon. **Cette réévaluation remonte MFT/USN dans l'ordre de travail produit**, tout en conservant les exigences de fiabilité de la spec. Les mentions anciennes « P1 » ou « later work » ne signifient donc pas ici « sans importance ».

Effort : **S** localisé, **M** plusieurs modules, **L** chantier important avec validation native. Un effort L ne justifie pas à lui seul une priorité basse. A ne signifie pas que tout doit commencer simultanément : les lots de livraison fixent l'ordre et les dépendances.

### Changements par rapport au classement précédent

| Sujet | Nouvelle décision | Pourquoi |
|---|---|---|
| **S09 : inventaire NTFS/MFT** | **C → A**, premier chantier du parcours disque entier. | La latence et la couverture conditionnent l'utilité du produit. Le besoin est établi ; le benchmark valide l'implémentation. |
| **S10 : index persistant et USN** | **A**, chantier séparé, juste après l'inventaire initial. | Revenir dans l'application doit être rapide. Ce besoin est plus proche que la sauvegarde complète d'enquêtes I03. |
| **S02 / U06 : navigation et interface minimale** | **B → A** pour les parcours essentiels. | L'utilisateur doit retrouver le fichier précis et comprendre ce qu'il sélectionne ; un top N et des options CLI ne suffisent pas à l'interface cible. |
| **S06 : première action utile** | **B → A**, activation après S03/S04 et recette dédiée. | Le produit doit aller jusqu'à une décision et un résultat vérifiable sur un cas ordinaire. Les adaptateurs supplémentaires restent B/C. |
| **U03 / U05 : exclusions et partage maîtrisé** | **A** pour les contrôles indispensables au parcours. | Choisir ce qu'on conserve et ce qu'on divulgue fait partie de l'usage normal. Les alias et l'opt-in de noms sont déjà livrés. |
| **Q05 / Q07 / Q08** | **A pour les capacités, contrats et contrôles requis par les nouveaux moteurs/IPC**. | MFT, USN et GUI rendent ces travaux nécessaires plus tôt ; pas de chantier abstrait préalable à toute fonction. |
| **D01–D03 / D06 / I01 / Q02–Q04** | **A maintenu**. | Vitesse du stockage et qualité du diagnostic se complètent. Une mesure instantanée reste insuffisante pour une lenteur intermittente. |
| **E02–E04 / E07** | **B pour les besoins ciblés**, C pour l'extension exhaustive. | Chauffe, contexte d'alimentation, « depuis hier » et lenteur réseau méritent un parcours utile sans viser tous les capteurs ou toutes les traces. |
| **S11 / S12 / E10 / E11** | Entrées explicites **B**. | Étendre les doublons, vérifier le contexte/récupération, relayer vers l'antivirus et contrôler l'intégrité étaient insuffisamment décrits. |
| **S05 / Q01** | Pré-scan et intégration **livrés**. | Leur travail restant est rattaché à I01/S10 et aux contrôles transversaux ; on ne redéveloppe pas ces acquis. |

<a id="comparaison"></a>

### Ce que les outils existants nous apprennent

[WizTree](https://diskanalyzer.com/about) utilise la MFT pour accélérer l'inventaire NTFS ; [WinDirStat](https://windirstat.net/) propose désormais aussi un scan NTFS direct lorsque disponible, avec navigation et vues de stockage. **Déduction produit :** attendre longtemps avant de voir les gros postes serait une faiblesse centrale de PC Agent. La vitesse et la lisibilité doivent être travaillées ensemble.

[Everything](https://www.voidtools.com/faq/) actualise ses index NTFS avec le journal USN. **Déduction produit :** il faut distinguer le coût du premier inventaire de celui des actualisations ; relancer un parcours complet à chaque question est un mauvais objectif d'architecture.

[System Informer](https://system-informer.com/), [RAMMap](https://learn.microsoft.com/en-us/sysinternals/downloads/rammap) et [HWiNFO](https://www.hwinfo.com/about-software/) offrent déjà des vues techniques approfondies. **Déduction produit :** PC Agent doit sélectionner les mesures utiles, les relier à la question et proposer la prochaine étape avec ses limites. Un catalogue de chiffres supplémentaire ne suffit pas.

[Storage Sense](https://support.microsoft.com/en-us/windows/experience/storage-filemanagement/manage-drive-space-with-storage-sense) et [PC Manager](https://pcmanager.microsoft.com/en-us) couvrent déjà des opérations courantes de stockage. [Czkawka/Krokiet](https://github.com/qarmin/czkawka) sépare notamment doublons, similarité et fichiers endommagés. **Déduction produit :** guider vers l'opération adaptée, réutiliser les outils compétents et garder ces diagnostics distincts.

L'[agent des paramètres Windows](https://support.microsoft.com/en-us/windows/experience/exploring-windows-settings), sur les machines prises en charge, et [ETW MCP](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/etw-mcp-early-preview-july-2026), en préversion, sont des repères pour l'assistance et l'investigation. La différenciation proposée est un parcours simple allant de la mesure à une décision vérifiée ; elle reste à éprouver auprès d'utilisateurs.

<a id="mft-usn"></a>

## 3. MFT et USN : un chantier central de stockage

| Mécanisme | Travail attendu | Priorité et dépendances |
|---|---|---|
| **Inventaire NTFS/MFT — S09** | Construire vite la liste des fichiers/dossiers, leurs relations et les métadonnées nécessaires au classement. | **A**, derrière l'interface `ScanBackend`, avec comptabilité, couverture et permissions vérifiées. |
| **Index local + journal USN — S10** | Conserver l'état connu puis intégrer les changements depuis le curseur enregistré, au lancement ou au rafraîchissement. | **A**, conçu avec S09 et livré ensuite ; contrats/persistance Q07, sans dépendance à l'historique des conversations I03. |
| **Parcours classique** | Couvrir les périmètres pris en charge sans accès accéléré, les petits dossiers et les autres systèmes de fichiers compatibles. | Backend conservé et testé. Son maintien ne relègue pas MFT à une option lointaine. |

La MFT sert à l'inventaire ; le journal USN sert au suivi des modifications du système de fichiers. Le **journal d'actions de PC Agent** décrit les opérations approuvées et leurs résultats. Ces trois objets ont des responsabilités différentes. [Microsoft : Change Journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals)

### Contrat proposé pour la mise en œuvre

- **Mesurer l'inventaire utile complet.** `FSCTL_ENUM_USN_DATA` permet une énumération liée à la MFT ; `USN_RECORD_V2` ne fournit pas toutes les tailles et allocations. Prévoir l'enrichissement nécessaire et mesurer son coût. Le choix entre interfaces Windows documentées et éventuelle lecture directe validée relève de l'implémentation ; « noms inventoriés » ne signifie pas « tailles entièrement connues ». [Énumération NTFS](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data), [structure des enregistrements](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2)
- **Limiter les privilèges à un module natif de lecture.** Prévoir une élévation ponctuelle consentie pour l'accès au volume, avec opérations fixes, appelant vérifié et périmètre local. L'interface et le modèle restent sans droits administrateur ; aucune commande libre, aucun service administrateur permanent requis. WizTree indique que son scan NTFS rapide exige l'administration ; la contrainte doit être traitée, pas servir à déprioriser la fonction. [FAQ WizTree](https://diskanalyzer.com/faq)
- **Respecter le périmètre réellement choisi.** Une lecture technique de métadonnées du volume ne vaut pas autorisation d'exposer tout le PC. Exclusions, liens, cloud et accès doivent garder une sémantique explicite. Filtrer avant indexation durable/export/modèle, y compris les changements de parents après déplacement. Le scan ne monte ni ne déverrouille un volume.
- **Garder une comptabilité correcte.** Distinguer tailles logiques, allocations, estimation de récupération et espace libre observé ; traiter les liens physiques et les recouvrements de scopes sans double comptage. Afficher l'espace non attribué, les enrichissements manquants et l'état du scan pendant sa progression.
- **Rendre l'index récupérable.** Associer volume, identité/incarnation de fichier, identifiant du journal et curseur ; publier ensemble l'état de l'index et sa position dans le journal. Traiter les changements survenus pendant le scan initial, les renommages de dossiers, fichiers supprimés/recréés, liens et interruptions. Les alias d'un ancien scan ne deviennent pas une identité durable ni une autorisation d'action.
- **Reconnaître les trous de journal.** Journal absent, remplacé, entrées expirées, volume absent ou schéma incompatible : signaler l'état périmé/incomplet et reconstruire ou utiliser le parcours classique. Ne pas créer, agrandir ou réinitialiser automatiquement le journal Windows. [Identifiant de journal](https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier), [curseur devenu trop ancien](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v0)
- **Rester local et contrôlable.** Persistance de l'index facultative, emplacement et taille visibles, purge et exclusions applicables à l'index déjà enregistré. Sans persistance, le scan rapide reste disponible. Le rafraîchissement à la demande n'impose pas un agent actif en permanence.
- **Conserver une observation datée.** MFT/USN ne transforme pas un système actif en snapshot immuable. Avant une action, vérifier de nouveau la cible et ses protections. L'index rapide alimente les alias et le contexte local déjà présents ; il n'exige pas de transmettre les vrais chemins au cloud.

### Acceptation et mesures

Comparer le backend classique et le backend accéléré sur un corpus stable documenté : petit dossier, grande arborescence, nombreux petits fichiers, gros fichiers, liens physiques, fichiers compressés/creux, Unicode, chemins longs, dossiers refusés et cas cloud. Ajouter un volume soumis à des créations, suppressions et déplacements pendant la lecture.

Publier, pour le même périmètre et la même complétude : **temps jusqu'au premier résultat exploitable, temps d'inventaire avec tailles, temps de réouverture/rafraîchissement, CPU, mémoire et E/S**, en distinguant cache froid/chaud et index neuf/existant. Les mesures incluent l'enrichissement et le module élevé. Expliquer les différences de totaux, sans exiger que les fichiers attribués représentent tous les octets occupés du volume.

Le benchmark est un critère de livraison de S09/S10, pas une étude préalable pour décider si l'utilisateur mérite un scan rapide. Aucun temps « comme WizTree » n'est annoncé comme acquis avant ces mesures. Les limites actuelles du pré-scan — 100 000 entrées, 64 Mio d'index estimé, 15 s par racine dans 30 s de préparation partagées — sont des plafonds produisant éventuellement un résultat partiel, pas une preuve de performance à l'échelle du PC.

Enfin, **MFT/USN ne remplace pas la lecture de contenu** nécessaire aux doublons confirmés, à l'antivirus ou à la validation de formats. L'inventaire sert à cibler et planifier ces opérations séparées.

<a id="backlog"></a>

## 4. Backlog complet

Les identifiants historiques sont conservés. « Partiel » indique qu'un socle existe, pas que le parcours utilisateur est terminé. Chaque critère de fin ci-dessous est une exigence proposée à vérifier lors du lot concerné.

### Stockage, fichiers et actions

| ID · priorité · effort | État et bénéfice | Travail restant et critère de fin |
|---|---|---|
| **S09 · A · L** | **Absent — inventaire rapide NTFS/MFT.** Voir vite les postes de stockage du disque entier. | Implémenter le backend selon le contrat ci-dessus. **Fini quand :** inventaire avec tailles exploitable, couverture explicite, annulation, repli et comparaison de résultats/performance publiés. Dépend des contrôles S03/S04, I01 et Q04/Q05/Q08 au fur et à mesure. |
| **S10 · A · L** | **Absent — index persistant et actualisation USN.** Retrouver et rafraîchir les résultats. | Index versionné, consentement de conservation, curseur transactionnel, gestion des interruptions et invalidations. **Fini quand :** changements depuis la fermeture correctement intégrés, index périmé jamais présenté comme actuel, purge/exclusions effectives et reconstruction testée. Suit S09, dépend de Q07, pas d'I03. |
| **S02 · A · M** | **Partiel — navigation et sélection.** Passer du volume au dossier puis au fichier exact. | Exposer arborescence, recherche/tri/filtres locaux, pagination, exclusions et sélection ; vrais noms localement, alias côté modèle. **Fini quand :** une cible hors du top N est retrouvable, les branches non inspectées sont visibles et parent/enfants ou scopes recouvrants ne gonflent pas le total sélectionné. U06 peut commencer sur le backend classique ; la treemap est un complément B. |
| **S03 · A · M** | **Écart confirmé — attributs transmis à la politique.** Éviter une classification fondée sur des attributs absents. | Dans `Workstation`, des `CandidateInput` utilisent encore `Attributes::default()` ; la projection de contexte doit aussi être alignée. Propager valeurs observées et inconnues jusqu'aux recommandations/plans. **Fini quand :** caché, système, cloud et non mesuré sont distingués dans le corpus. Requis avant S06 ; ce constat ne démontre pas une suppression dangereuse dans le catalogue actuel. |
| **S04 · A · M** | **Partiel — dossiers Windows et cloud.** Comprendre les vrais emplacements et ce que le scan couvre. | Remplacer les seules déductions d'environnement de `KnownFolders::from_env` par une résolution Windows adaptée ; vérifier dossiers déplacés et fichiers OneDrive en ligne/hydratés. **Fini quand :** aucun rappel de contenu ni élargissement de scope, parties ignorées expliquées, suppression synchronisée distincte du retrait d'une copie locale. L'attribution sûre des fichiers hydratés peut suivre en B ; la couverture honnête est A. |
| **S01 · A · M** | **Partiel — récupérer X Go.** Transformer l'inventaire en choix concrets. | Exposer `Policy::build_plan` dans CLI/agent/GUI avec objectif, périmètre et exclusions. **Fini quand :** objectif atteignable ou impossible expliqué, octets observés/estimés distingués, récupération immédiate/différée séparée ; déplacement vers la corbeille non présenté comme espace immédiatement libéré. Aucun fichier personnel ajouté pour atteindre artificiellement l'objectif. |
| **S06 · A · L** | **Partiel — première action utile sur données ordinaires.** Aller jusqu'à un résultat réel. | Première tranche : guidage vers un outil Windows adapté et sélection locale étroite pour la corbeille ; fermeture normale seulement avec cible et contexte vérifiés. **Fini quand :** cible exacte, effets, possibilité réelle de récupération, revalidation, annulation, journal et bilan sont vérifiés de bout en bout sur données jetables. Activation après S03/S04 et recette dédiée ; adaptateur de vrai cache en B, sans nettoyage générique d'AppData/Temp. |
| **S05 · Livré · —** | **Pré-scan des dossiers autorisés.** Éviter les tours modèle dépensés à démarrer un scan. | `diagnose --scan-root` prépare les index bornés, y compris hors ligne/aperçu, et conserve leurs limites. Prolongations et jobs existent. Restes rattachés à **I01** pour le contrôle unifié et **S10** pour la réutilisation : pas de deuxième implémentation de pré-scan. |
| **S07 · B · M** | **Absent — qu'est-ce qui a grossi ?** Comparer deux états compréhensibles. | Conserver explicitement des points de comparaison, dates, périmètres et exclusions. **Fini quand :** création, croissance, déplacement, disparition et changement de couverture sont distingués ; une comparaison non fiable est refusée. Dépend de S10/Q07 ; un index courant et son curseur seuls ne conservent pas l'ancien état à comparer. |
| **S08 · B · M** | **Partiel — gros postes développeur.** Comprendre WSL/Docker, VM, modèles et builds. | Règles d'explication dédiées, association locale prudente et relais vers les outils officiels. **Fini quand :** taille d'un VHD hôte et espace libre invité sont distincts ; projet, image, volume et cache ne sont pas confondus. Aucun VHD/volume Docker supprimé sur sa seule taille. |
| **S11 · B · L** | **Partiel — doublons vérifiés et choix de copie.** Étendre `--compare-content` au-delà du sous-ensemble actuel. | Comparaisons entre scopes autorisés, sélection guidée, budgets visibles et reprise ; cache de résultats invalidé quand l'identité/contenu change. **Fini quand :** mêmes tailles avec contenus différents, liens physiques, fichiers modifiés, copies nécessaires et chemins exclus sont traités ; conserver une copie est un choix explicite. Dossiers identiques = arborescences et fichiers entièrement couverts, sinon inconnu. Similarité de photos/vidéos en C ; aucune déduplication ou suppression automatique. |
| **S12 · B · M** | **Partiel — application associée et récupération vérifiées.** Mieux répondre « puis-je supprimer ceci ? ». | Croiser règles de chemin avec installation/manifestes disponibles et documenter les conditions de restauration, régénération ou téléchargement. **Fini quand :** correspondance probable, propriété vérifiée et inconnue sont distinctes ; sauvegarde/copie encore accessible et récupération testée ne sont pas confondues. Le contexte livré reste utile mais ne prouve ni inutilité ni permission de supprimer. |

Points d'appui : [scanner](../crates/storage/src/lib.rs), [contexte agent](../crates/agent/src/storage_context.rs), [Workstation](../crates/agent/src/workstation.rs), [politique](policy.md), [plans](../crates/policy/src/plan.rs), [actions](actions.md).

### Diagnostic, explications et décisions

| ID · priorité · effort | État et bénéfice | Travail restant et critère de fin |
|---|---|---|
| **D01 · A · M** | **Partiel — observation guidée.** Capturer la lenteur pendant qu'elle survient. | Ajouter une capture à durée/cadence bornées, marqueur « ça rame ici », lacunes et contexte de charge, sans cloud. **Fini quand :** la fenêtre du symptôme est retrouvable ou déclarée manquante, y compris si un processus disparaît ; arrêt propre via I01. L'historique actuel ne se remplit qu'au rythme des collectes. |
| **D02 · A · M** | **Partiel — findings sur plusieurs échantillons.** Distinguer pic bref, croissance et pression persistante. | Calcul local de durée, dispersion, tendance et temps sous pression, avec lacunes explicites. **Fini quand :** corpus normal/pic/charge soutenue différencié, répétition d'un échantillon sans renforcement artificiel ; une tendance ne prouve pas à elle seule une fuite. Dépend de D01/Q02. |
| **D03 · A · M** | **Partiel — mode local adapté à la demande.** Rester utile sans modèle. | Le pré-scan hors ligne existe, mais `local_report(&initial)` ne choisit pas une enquête selon la question. Ajouter intentions explicites mémoire/CPU/stockage/capture puis conclusion locale exploitant les observations pertinentes. **Fini quand :** les parcours principaux répondent au besoin choisi sans clé ni requête réseau ; aucune promesse de compréhension libre hors ligne. |
| **D06 · A · M** | **Partiel — réponse compréhensible et prochaine étape.** Relier faits, hypothèses et décisions. | Réutiliser les citations et trois statuts déjà livrés ; ajouter conclusions métier, dates et couverture pertinente pour la question. **Fini quand :** RAM non attribuée, scan incomplet, objectif impossible, symptôme non capturé et besoin de nouvelle mesure donnent des suites distinctes ; pas de « PC sain » déduit d'une exécution terminée. |
| **D04 · B · M** | **Partiel — regroupement par application.** Comprendre navigateur, IDE ou Docker plutôt qu'une liste de processus. | Regrouper avec sources/ambiguïtés, relier localement une application citée aux références opaques. **Fini quand :** instances distinguées, nom réel lisible localement et mémoire partagée non additionnée comme RAM exclusive. Le tri CPU/E/S et l'arbre existent déjà. |
| **D05 · B · M** | **Partiel — clarifications et contraintes.** Poser les questions qui changent l'enquête. | Symptôme, début, charge à conserver, batterie/secteur et périmètre structurés. **Fini quand :** « garde Docker » ou « seulement ce dossier » devient une contrainte appliquée ; les demandes d'information ne valent pas approbation d'action. Dépend d'U03/I02. |
| **D07 · B · M** | **Partiel — vérification représentative.** Vérifier le bénéfice dans l'usage réel. | Allonger les fenêtres utiles avant/après, conserver le contexte de charge et permettre « non comparable ». **Fini quand :** baisse mémoire, espace gagné et fluidité restent des mesures distinctes ; critère technique et ressenti sont séparés. La vérification par type d'action existe déjà et reste obligatoire pour S06. |
| **D08 · B · M** | **Partiel — conseil matériel conditionnel.** Choisir entre usage, entretien et achat. | Croiser contraintes répétées et applications nécessaires ; distinguer capacité installée, besoin mesuré et compatibilité inconnue. **Fini quand :** cas « ne rien acheter », « mesurer davantage » et « capacité à étudier » présents ; aucune taille conseillée à partir du seul commit. Dépend de D01/D02/D05/Q03. |

Points d'appui : [collecteur](../crates/telemetry/src/lib.rs), [historique](../crates/telemetry/src/history.rs), [diagnostics](../crates/diagnostics/src/lib.rs), [comparateur](../crates/diagnostics/src/compare.rs), [CLI](../apps/cli/src/main.rs).

### Enquête, continuité et coût

| ID · priorité · effort | État et bénéfice | Travail restant et critère de fin |
|---|---|---|
| **I01 · A · M** | **Partiel — progression et interruption unifiées.** Savoir ce qui se passe et pouvoir arrêter. | Unifier contrôle de collecte, scan, comparaison de contenu et attente cloud ; conserver bilan et observations. **Fini quand :** annuler empêche les nouveaux travaux, clôt les jobs et rend un rapport partiel cohérent ; les appels natifs non interruptibles sont signalés. Annulation des scans/actions et jointure existent déjà. |
| **I02 · B · M** | **Absent — questions de suivi.** « Et ce fichier ? », « refais une mesure », « seulement D: ». | Conversation utilisant les références et index de la session, avec fraîcheur des observations et contraintes vérifiées. **Fini quand :** une suite simple ne redémarre pas toute l'enquête ; ancienne mesure actualisée si nécessaire ; tout nouveau scope autorisé localement. La prolongation de budget n'est pas encore ce parcours. |
| **I03 · B · L** | **Absent — historique facultatif et reprise d'enquête.** Retrouver une session après fermeture. | Stockage versionné, rétention, export/purge et séparation fiable des boots pour les processus. **Fini quand :** reprise sans secret/consentement d'action réutilisé, références et plans revalidés, aucune action rejouée. Dépend de Q07/U05 ; reste distinct de l'index stockage S10 et du journal d'actions existant. |
| **I04 · B · M** | **Partiel — budgets utilisateur de coût et délai.** Choisir la profondeur de l'enquête. | Utiliser le suivi de tokens/coûts et les prolongations déjà intégrés ; ajouter plafonds configurables, cumul visible et estimation prudente de la suite. **Fini quand :** contrôle avant nouvel appel, usage inconnu jamais annoncé gratuit, estimation client distincte de facture fournisseur. Pas de plafond monétaire exact promis sans borne vérifiable. |
| **I05 · B · M** | **Partiel — erreurs cloud récupérables.** Garder le bénéfice des observations locales. | Différencier configuration/authentification/réseau/limitation ; reprises bornées selon l'erreur et retour local utile. **Fini quand :** bilan conservé, suite compréhensible, pas de boucle de retries ni de rejeu d'action, fournisseur jamais changé implicitement. |

Points d'appui : [agent](../crates/agent/src/lib.rs), [transport](../crates/agent/src/nebius.rs), [journal d'actions](../crates/actions/src/journal.rs).

### Interface, confidentialité et distribution

| ID · priorité · effort | État et bénéfice | Travail restant et critère de fin |
|---|---|---|
| **U06 · A · L** | **Absent sur main — interface Tauri minimale.** Rendre les deux parcours utilisables sans options CLI. | Choix du besoin/scopes, capture/progression/annulation, arbre stockage avec vrais noms, preuves/limites, sélection et confirmation d'action. **Fini quand :** mêmes décisions et protections que la CLI, clavier/lecteur d'écran, français/anglais et statuts compréhensibles sans couleur ; aucun terminal helper intempestif. Réutiliser le Core Rust ; historique avancé et finitions suivent en B. |
| **U02 · A · M** | **Partiel — premier lancement et doctor.** Comprendre les capacités réellement disponibles. | Détecter version/architecture Windows, volumes et backends, permissions, capteurs et raisons d'indisponibilité via Q05. **Fini quand :** premier résultat local sans Rust ni clé, choix du périmètre compris, accès MFT expliqué avec repli ; pas d'invitation générale à exécuter toute l'app en administrateur. |
| **U03 · A · M** | **Partiel — préférences accessibles.** Protéger dossiers/applications et conserver ses choix. | Interface guidée des exclusions et applications préservées, validation et aperçu d'effet. **Fini quand :** préférence modifiable appliquée aux scans/index, recommandations et actions ; permissions de contenu/cloud/noms/persistance distinctes. Les fichiers JSON actuels restent compatibles. |
| **U05 · A · M** | **Partiel — contrôle du partage.** Comprendre et choisir ce qui quitte le PC. | Partir des alias et de la liste finie de noms déjà livrés ; ajouter sélection manuelle, aperçu dans la même session et catégories d'envois ultérieurs. **Fini quand :** noms non cochés absents, rescans/révocations ne rétablissent pas un partage, question sensible éditable et aucune transmission d'historique complet implicite. Journal local expurgé des échanges en B ; aucune promesse d'anonymat. |
| **U01 · B · M** | **Absent — export support expurgé.** Partager un diagnostic sans publier l'inventaire personnel. | Export Markdown/HTML/JSON dédié, aperçu, choix des observations et expurgation des chemins/noms/question/secrets. **Fini quand :** version, couverture et faits restent utiles ; exporter ne transmet rien. À livrer avant un test externe qui exige le partage de rapports ; JSON local brut non présenté comme partageable. |
| **U04 · B · M** | **Absent — coffre Windows pour la clé.** Configurer/remplacer/supprimer le secret sans fichier en clair. | Intégrer le coffre, provenance affichée et erreurs claires ; conserver une méthode de configuration documentée. **Fini quand :** aucun secret dans exports/journaux, suppression effective et migration volontaire du fichier utilisateur. À traiter avant diffusion cloud large. |
| **U07 · A puis B · M/L** | **Partiel — distribution testable puis durable.** Essayer l'app sans environnement de développement. | **A :** binaire/version/commit identifiables, empreinte, instructions courtes et retrait des données. **B :** installateur, signature et mises à jour vérifiées. **Fini quand :** testeur sur autre PC lance puis retire l'app et ses données. Licence et compatibilité des dépendances à trancher avant publication ; aucun secret partagé dans le binaire. |

U01 passe après les contrôles nécessaires au parcours local : le premier scan/diagnostic doit être utilisable avant de disposer de plusieurs formats d'export. Cela ne dispense pas d'expurger les rapports demandés aux testeurs.

### Qualité, contrats et codebase

| ID · priorité · effort | État et bénéfice | Travail restant et critère de fin |
|---|---|---|
| **Q01 · Livré · —** | **Intégration des branches.** Conserver un socle cohérent. | NVML/coûts et prolongations/citations/statuts réunis par #14 ; contexte stockage intégré par #15. Les régressions restent suivies dans chaque lot ; cette fusion n'est plus une tâche ouverte. |
| **Q02 · A · M** | **Partiel — corpus et replay produit.** Mesurer la justesse des décisions. | Scénarios normaux, pics, contention, ambiguïtés, données manquantes, contraintes et stockage protégé ; replay déterministe puis comparaison séparée des modèles autorisés. **Fini quand :** faits erronés, conclusions trop fortes, abstentions utiles, actions bloquées, délai et coût sont suivis. Commence avec D01/S09. |
| **Q03 · A · M** | **À compléter — recettes Windows réelles.** Vérifier au-delà du PC de développement. | Matrice sans NVIDIA, portable hybride, autres volumes/profils et compte standard, avec refus d'élévation/accès. **Fini quand :** capacités attendues/observées et cas non essayés publiés pour le commit livré ; fixtures jetables pour les effets. CI Windows ne remplace pas cette recette. |
| **Q04 · A · M** | **À démontrer — vitesse et empreinte totale.** Ne pas aggraver le problème observé. | Mesurer app, UI, index et helpers : CPU/RAM/E/S, délais, scan initial/incrémental et réveil GPU ; contexte batterie. **Fini quand :** chiffres avec corpus/machine/charge, propres processus visibles et budgets adaptés ; pas d'annonce de performance basée seulement sur des timeouts configurés. |
| **Q05 · A · M** | **Partiel — capacités partagées.** Aligner doctor, outils, rapport et interface. | Centraliser support, permissions, coût de collecte et motif d'indisponibilité ; distinguer implémenté/exposé/validé. **Fini quand :** même réponse pour backend MFT indisponible, journal absent, inventaire GPU incomplet et capteur NVML disponible ; une seule source de vérité du runtime. |
| **Q07 · A pour index/IPC · M** | **Partiel — contrats versionnés.** Éviter des données illisibles ou des tailles fausses. | Définir migrations/invalidation de l'index et contrats IPC ; garder octets 64 bits exacts côté JavaScript. **Fini quand :** anciens formats migrés ou refusés clairement, aucune perte silencieuse de précision, JSON CLI unique et codes de sortie documentés. Session historique complète reste I03/B. |
| **Q08 · A ciblé, puis B · M** | **Partiel — contrôles des nouvelles surfaces sensibles.** Préserver les garanties existantes. | Tests de parseurs NTFS/USN, longueurs/versions, changements d'identité, IPC du module élevé et données hostiles avant S09/S10/U06 ; fuzzing/génératif ciblé quand utile. **Fini quand :** régressions reproductibles en CI, repli sûr et aucune action d'effet sur données utilisateur ; suivi des dépendances/licences avant distribution. |
| **Q06 · Transversal · M** | **Continu — refactors ciblés.** Garder les évolutions locales et testables. | Budget/rapport, projection stockage et comparaison de contenu sont déjà séparés. Extraire capture, index durable et orchestration de session au rythme des besoins ; garder règles dans diagnostics/policy, autorisations/actions dans leurs modules et UI mince. **Fini pour chaque lot quand :** aucun doublon de validation/consentement, interface courte et scénarios comportementaux lisibles. Pas de réécriture générale ni framework de plugins spéculatif. |

Le scanner classique et le futur backend NTFS doivent partager les mêmes contrats de résultats et de couverture. L'index masque la gestion du journal derrière une interface simple ; le CLI, la GUI et le modèle n'ont pas à reconstruire chacun les règles de fraîcheur, d'identité ou de permission.

### Extensions utiles après les premières tranches

| ID · priorité · effort | Bénéfice et état | Périmètre retenu et critère de fin |
|---|---|---|
| **E01 · B · L** | **Attribution disque/GPU plus précise.** Les compteurs actuels sont surtout agrégés. | Identifier appareil/moteur/processus quand la source le permet. **Fini quand :** provenance et couverture visibles, E/S de processus non assimilées à des accès disque physiques, décision améliorée sur des cas du corpus. |
| **E02 · B ciblé, C exhaustif · L** | **Capteurs complémentaires utiles au diagnostic.** CPU thermique/fréquence, GPU non NVIDIA, stockage. | Commencer par les observations qui peuvent expliquer un symptôme et dont l'accès est vérifiable. **Fini quand :** lecture comparée à une référence sur matériel réel, coût/privilèges mesurés et absence explicite. Pas de pilote ajouté seulement pour augmenter la couverture ; SMART isolé ne prédit pas une panne. |
| **E03 · B · M** | **Batterie, chauffe et veille.** Répondre à un besoin portable. | D'abord contexte secteur/batterie et import local borné de rapports adaptés ; approfondissement du sommeil ensuite. **Fini quand :** support/privilèges expliqués et fenêtres comparables ; aucune modification automatique du plan d'alimentation. Dépend de D01/D02, éventuellement E02. |
| **E04 · B · M/L** | **« C'est lent depuis hier », démarrage et plantages.** Rendre la chronologie utile. | Inventaire de démarrage/services en lecture seule, événements/versions ciblés et datés, sans aspirer tous les journaux. **Fini quand :** absence d'historique distincte d'absence d'événement, changement proche du symptôme présenté comme piste ; coût de boot seulement avec mesure adaptée. Pas de désactivation automatique. |
| **E05 · C · L** | **Relais WPR/WPA et traces spécialisées.** Résoudre les cas que les compteurs ordinaires ne tranchent pas. | Profils connus, durée/taille bornées, privilèges explicites, analyse locale ou export contrôlé ; étudier ETW MCP comme complément. **Fini quand :** le résultat revient dans un rapport compréhensible avec limites. Aucun ETL brut envoyé par défaut, aucun shell modèle. |
| **E06 · C · M/L** | **Fluidité en jeu ou tâche précise.** Mesurer le symptôme directement. | Temps d'image/latences ou durée de tâche avec outil adapté, charge comparable et opt-in. **Fini quand :** gain lié à une mesure réelle et restrictions du collecteur documentées. PresentMon est une référence ; pas de gain FPS extrapolé d'une baisse de RAM. |
| **E07 · B pour triage, C approfondi · M/L** | **Lenteur réseau ou service distant.** Orienter correctement l'enquête. | Distinguer symptômes local/connexion/application ; état réseau local, puis tests actifs bornés vers une destination autorisée. **Fini quand :** domaines non mesurés explicites et aucune capture de contenu ou requête Internet implicite. Pas de moniteur réseau complet dans la première tranche. |
| **E08 · C · M** | **Recherche documentaire sourcée.** Compléter un problème déjà identifié. | Tavily non intégré ; ne partager que composant/version utiles, avec permission distincte, sources officielles datées et contenu traité comme non fiable. **Fini quand :** conseils applicables et cités, sans causalité déduite d'une ressemblance ni commande arbitraire. |
| **E09 · C · L** | **Alertes et références personnelles.** Prévenir un problème récurrent. | Après D01/D02/I03/Q02/Q04 : collecte/démarrage opt-in, rétention, temporisation et budget. **Fini quand :** faux positifs mesurés et notifications utiles, désactivables ; aucune action automatique pour justifier une alerte. |
| **E10 · B · M** | **Absent — triage antivirus via Windows.** Répondre aux soupçons sans inventer un antivirus IA. | Commencer par état/provenance/date de protection et relais vers Sécurité Windows ; scan Defender seulement comme opération fixe, explicitement autorisée, avec effets potentiels de quarantaine expliqués. **Fini quand :** résultat et couverture rapportés, antivirus tiers/Defender indisponible reconnus ; aucun verdict depuis un nom, aucune suppression/quarantaine décidée par le modèle. |
| **E11 · B ciblé, C généralisé · M/L** | **Absent — fichiers potentiellement corrompus.** Distinguer accès impossible, format invalide et incohérence vérifiée. | Lectures locales consenties et validateurs pour quelques formats annoncés ; intégrité des fichiers système via outils Windows séparés. **Fini quand :** preuve de l'erreur, format inconnu/chiffré non déclaré corrompu, plafonds de temps/taille/mémoire contre fichiers hostiles. Un fichier de zéro octet ou d'extension inhabituelle n'est pas une preuve ; aucune réparation/suppression automatique. |

Doublon, similarité, menace et corruption sont quatre conclusions différentes. Un hash identique ne prouve ni qu'une copie est inutile ni qu'elle est saine ; un hash différent ne prouve pas une corruption. Lire de grandes quantités de contenu peut coûter beaucoup plus que l'inventaire MFT.

<a id="livraison"></a>

## 5. Ordre de livraison recommandé

Deux axes peuvent avancer indépendamment : le stockage rapide et le diagnostic sur une durée. Si une seule tranche est menée à la fois, **commencer par le stockage rapide**, conformément au besoin exprimé, avec progression et contrôles dès le début. Le chantier MFT n'attend pas que toutes les améliorations de diagnostic soient terminées.

| Lot | Livrable démontrable | Dépendances et sortie attendue |
|---|---|---|
| **0 — État intégré et contrats** | Partir du main actuel, conserver les acquis et définir le corpus du prochain lot. | Q01 déjà livré ; pas de nouvelle phase de fusion à inventer. Démarrer S03, Q02/Q04/Q05/Q07 au niveau nécessaire aux lots suivants. |
| **1S — Inventaire rapide exploitable** | Volume autorisé → scan NTFS → premiers résultats → arborescence locale → couverture et fin/annulation. | S09, S02, S03/S04, I01, tranche stockage U06/U02 ; Q03/Q04/Q08. Une CLI permet de valider le moteur avant la GUI ; ne pas attendre l'interface complète pour le benchmark. |
| **2S — Revenir sans tout rescanner** | Fermer → rouvrir l'index → appliquer les changements USN → naviguer dans l'état actualisé. | S10/Q07 et contrôles de persistance U03/U05 ; journal remplacé, trou, volume absent et interruption testés. Enchaîne directement 1S, indépendant d'I03. |
| **1D — Observer une vraie lenteur** | Capture locale → marqueur → analyse de fenêtre → conclusion ou prochaine mesure. | D01/D02/D03/D06, I01 et tranche capture U06 ; Q02–Q04. Peut avancer parallèlement à 1S/2S sans attendre le moteur NTFS. |
| **3 — Décider et agir utilement** | Objectif « X Go » → choix/exclusions → action précise consentie → résultat mesuré. | S01/S06, U03/U05, revalidation S03/S04 et recette dédiée. Préparer les plans dès le scanner classique ; activer l'effet seulement lorsque le parcours et ses protections passent. |
| **4 — Mise à l'essai du produit** | Binaire repérable, deux parcours utilisables, export expurgé et retours de PC distincts. | U01/U02/U06/U07, Q02–Q04. Les recettes commencent dès les premiers lots ; ce jalon rassemble les preuves avant diffusion plus large. |
| **5 — Approfondir les besoins courants** | Choisir des tranches B : suivi de question, croissance du disque, doublons, contexte/récupération, matériel, réseau, antivirus ou intégrité ciblée. | I02–I05, D04/D05/D07/D08, S07/S08/S11/S12, U04 et extensions B. Livrer par problème concret, selon dépendances et retours ; pas toutes les fonctions simultanément. |
| **6 — Spécialiser** | Traces expertes, fluidité en jeu, recherche documentaire, alertes ou similarité multimédia. | Extensions C après un cas non résolu et un critère d'utilité vérifiable ; aucun nouveau catalogue sans besoin. |

**Le prochain incrément concret est 1S :** backend NTFS + tailles fiables + progression/annulation + navigation locale, avec validation comparative. L'index et le curseur USN sont conçus dans cette tranche puis livrés en 2S. Une première capture D01 peut être développée séparément ; elle ne doit pas repousser l'inventaire rapide.

### Critères produit communs

- **Temps jusqu'à une décision utile :** première information exploitable, inventaire complet quand possible, réouverture, durée d'enquête et coût ; pas seulement temps de la première requête technique.
- **Qualité :** faits exacts, interprétations proportionnées, abstentions pertinentes et prochaine étape compréhensible.
- **Contrôle :** périmètre, exclusions, contenu lu, noms partagés, conservation locale et action exécutée réellement compris.
- **Effet :** résultat d'opération, espace libre, performance et ressenti séparés ; bénéfice vérifié sur une charge comparable.
- **Empreinte :** CPU/RAM/E/S de tous les processus, dont UI et helpers ; comportements sous contention et sur batterie.
- **Portabilité Windows :** capacité observée sur plusieurs machines et comptes ; cas non couverts nommés.

Le premier panel doit inclure développeurs, joueurs et utilisateurs de portables sur leurs tâches habituelles. Aucun entretien n'est revendiqué ici. Vérifier s'ils trouvent une cible, comprennent fait/hypothèse/limite, protègent ce qu'ils veulent garder et choisissent la suite sans assistance. Comparer règles seules et agent avec outils sur les mêmes cas ; les étoiles GitHub, votes Reddit et formulations séduisantes ne mesurent pas la qualité.

<a id="limites-produit"></a>

## 6. Limites : couvertes, volontaires et à lever

**Covered limitation** : une limite connue est détectée ou représentée honnêtement dans le résultat. La fonction manquante reste manquante. **Wanted limitation** : une restriction volontaire, durable ou propre au prototype. Une même fonction peut être implémentée, partiellement couverte et soumise à une restriction volontaire.

### Wanted limitations durables

| Restriction volontaire | Ce qu'elle protège et permet |
|---|---|
| Modèle limité aux outils et références validés localement | Aucun shell arbitraire, chiffre inventé, privilège ou autorisation d'effet accordé par le modèle. |
| Consentement local lié à une action précise | Manifeste/revalidation, pas de reprise automatique d'une action ; JSON/non-interactif n'approuve rien. |
| Métadonnées seules pour le scan par défaut | Contenu lu seulement par un parcours explicitement autorisé, comme `--compare-content`. Le scan MFT garde ce contrat. |
| Alias par défaut et vrais noms dans l'interface locale | Partage optionnel de noms utiles, borné et annoncé ; contenu et empreintes de comparaison restent locaux. Une question libre peut elle-même contenir des données sensibles. |
| Cloud, index durable, historique et surveillance permanente facultatifs | Choix indépendants et réversibles ; la persistance de l'index proposée par S10 ne rend pas obligatoires l'historique des enquêtes ou un service permanent. |
| Inconnus et périmètres explicites | Échec de collecte distinct d'une valeur nulle, date de modification distincte de dernière utilisation, taille distincte de gain récupérable. |
| Protection des fichiers personnels et système | Ancien, volumineux, dupliqué ou régénérable ne signifie pas supprimable. Copie identique distincte d'une sauvegarde restaurable. |
| Privilèges minimaux et opérations fixes | Un module élevé de lecture NTFS peut être ajouté après validation ; il n'accorde pas un shell administrateur à l'agent ni des droits à toute l'app. |
| Configuration du fournisseur depuis le profil/environnement | Le `.env` du répertoire courant reste ignoré ; sa présence ne permet pas de rediriger la configuration d'une exécution. |

### Wanted limitations du prototype, réexaminables

| Restriction actuelle | Évolution cohérente |
|---|---|
| Windows comme plateforme native ; CLI disponible | Garder cette cible, ajouter Tauri sans dupliquer les règles. Les tests portables Linux ne promettent pas une application native Linux. |
| Aucun chemin élevé implémenté | Lever précisément ce manque pour S09/S10 via le module de lecture contrôlé ; ne pas conserver « aucune élévation » comme interdiction durable du scan rapide. |
| Nettoyage permanent limité au cache synthétique ; catalogue étroit | S06 doit apporter un premier parcours réel testé. Pas d'élargissement générique pour remplir le catalogue. |
| Fournisseur/modèle encadrés par le parcours Nebius actuel | Évaluer les modèles autorisés sur coût/latence/qualité ; pas de changement implicite. Un modèle local est une piste ultérieure, distincte du mode hors ligne à règles. |
| Plafonds de scans, contenu et noms partagés | Garder des budgets, faire évoluer les valeurs et leur contrôle selon les usages mesurés. Les nombres actuels ne sont pas des limites produit intangibles. |
| Pas de réparation automatique universelle | Triage, comparaison, antivirus et intégrité peuvent progresser avec outils spécialisés ; aucune suppression ou réparation décidée sur un soupçon. |

### Covered limitations et travail associé

| Limite actuelle explicitée | Conséquence et chantier |
|---|---|
| Mesures courtes, seuils prototypes et historique alimenté lors des collectes | Pas de causalité, fuite ou conseil d'achat établis par un seul instant. D01/D02/D07/D08. |
| Processus inaccessibles, top N, premier delta inconnu et capteurs manquants | Couverture partielle, pas absence de consommation ; D04/E01/E02/Q05. |
| Disque/GPU souvent agrégés ; E/S de processus incluant cache/réseau | Pas d'attribution physique ou causale par processus à partir de ces seuls chiffres ; E01. |
| Scanner classique sans index durable ni USN | Le scan rapide et la réutilisation restent à développer en A ; reconnaître l'absence ne la résout pas. S09/S10. |
| Pré-scan tronqué, volumes/scopes non inspectés et fichiers cloud ignorés | Rapporter les exclusions et l'espace non attribué ; S02/S04/S09. Un résultat partiel ne devient pas un inventaire de tout le PC. |
| Scan non atomique, allocations inconnues et budgets coopératifs | Mutation, appel natif bloqué, tri final ou coût réel mémoire peuvent dépasser les hypothèses ; I01/Q04. MFT/USN ne supprime pas toute course. |
| Contexte application/récupération déduit de règles | Installation, propriété, corbeille disponible et sauvegarde ne sont pas vérifiées ; S12. |
| Comparaison actuelle limitée aux candidats de même taille parmi les 256 plus gros fichiers indexés par scope, avec 256 Mio/15 s par défaut | Pas de recherche exhaustive, pas de comparaison entre scopes/dossiers, aucune unicité conclue pour un fichier non comparé. Préparation `diagnose` : deadline et volume de lectures partagés entre scopes. S11. |
| Alias/`item_ref` liés à un scan | Résolution locale précise pendant le scan, pas identité persistante entre scans ; S10/Q07/I02. |
| Noms partagés : au plus 200 basenames sélectionnés automatiquement, racine exclue | Aperçu et envoi sont deux invocations possibles avec scans distincts ; le run cloud affiche sa liste réelle. Rescans sans extension implicite de la liste. Sélection fine et aperçu de la même session restent U05. |
| Corbeille Windows passant par chemin/PIDL après relâchement du pin | Fenêtre de substitution résiduelle contrôlée et vérification après effet, pas atomicité promise. S06 doit garder cette limite visible ; [détails](actions.md#recycle-binding-spec-113). |
| Octets supprimés différents de la variation d'espace libre | Allocation, corbeille et activité concurrente expliquent des écarts ; S01/D07. |
| Identité de boot fiable absente | Pas de corrélation persistante de processus entre boots ; pas d'action si identité non confirmée. I03. Ce manque ne bloque pas un index stockage fondé sur ses propres identités de volumes/fichiers. |
| Budget initial parfois insuffisant | Prolongation consentie déjà livrée : +6 tours, +12 outils, +90 s, au plus trois fois ; contexte conservé et temps humain exclu. JSON/non-interactif : bilan partiel avec `continuation`, sans extension automatique. Reprise après fermeture : I03. |
| Validation limitée au corpus et aux machines essayées | Tests automatisés et essais live existants ne constituent pas un benchmark de vitesse, une validation de tous les capteurs ou une recette utilisateur indépendante ; Q02–Q04. |

### Pistes à écarter du cœur produit

Booster de RAM, nettoyeur de registre, nettoyage universel d'AppData, suppression de fichiers personnels pour atteindre un objectif, désactivation automatique des protections, modifications pilotes/BIOS/overclocking, achats automatiques, score unique de santé et shell administrateur IA contredisent les garanties ou n'apportent pas une décision justifiée.

Gestion distante de parc, support natif multiplateforme et moteur antivirus maison diluent le périmètre actuel. Les signaler ne revient pas à rejeter le triage Defender, les capteurs utiles ou le contrôle d'intégrité ciblé. La vitesse d'inventaire et le suivi USN, eux, appartiennent au cœur stockage.

<a id="revue-initiale"></a>

## 7. Suivi de la revue technique initiale

Le document local `pc-agent-review.md` reste la revue d'origine. Le tableau suivant suit ses constats sur la base intégrée indiquée en tête. **Corrigé** signifie que le comportement a été modifié et sa régression ciblée traitée ; cela n'étend pas la validation à toutes les machines.

| Constat initial | État actuel et suite |
|---|---|
| 1. Cleanup non lié au contenu approuvé | **Corrigé** : manifeste, identités du répertoire/marqueur, revalidation avant effet et suppression Windows par handle vérifié. Inventaire modifié, racine remplacée ou manifeste trop long : refus. |
| 2. `reversible_only` inversé | **Corrigé** : `true` par défaut, même dans des préférences incomplètes ; opt-in distinct pour le cache synthétique. |
| 3. Deux JSON sur stdout | **Corrigé** : un document avec investigation, actions et recovery ; manifeste local des plans non exécutés conservé. |
| 4. Tout outil satisfait l'inspection obligatoire | **Corrigé** : seules les observations utilisables d'une inspection réussie débloquent la conclusion ; contexte et proposition ne suffisent pas. |
| 5. Observations absentes de la conclusion | **Corrigé** : `observation_ids` matériel/stockage/historique validés, faits rendus localement et `observation_refs`. Conclusions métier à enrichir : D06. |
| 6. Findings obsolètes sélectionnables | **Corrigé** : mesures fraîches comparables remplacent les findings concernés ; anciens conservés séparément, non sélectionnables comme actuels. |
| 7. Processus présélectionnés seulement par RAM | **Corrigé** : tri avant limitation par ressource, inconnus en dernier. Findings CPU/disque/GPU ponctuels présents ; tendances et attribution fine restent D02/E01. |
| 8. Énumération d'un gros dossier hors budget | **Corrigé** : parcours progressif et buffers bornés ; budgets coopératifs/estimation mémoire restent des covered limitations. |
| 9. Workers détachés à la fin | **Corrigé** : annulation et jointure, y compris `Drop` ; appel natif bloqué susceptible de retarder la fin. Contrôle global : I01. |
| 10. Plans perdant leurs preuves | **Corrigé pour la traçabilité** : références aux observations antérieures conservées ; elles ne prouvent pas seules la causalité ou la pertinence du remède. |
| 11. Cleanup partiel sans bilan | **Corrigé** : effets déjà constatés, fichiers et octets supprimés conservés malgré erreur ou annulation ultérieure. |
| 12. Dernier écrit de journal en échec masqué | **Corrigé** : erreur du journal empêche le résultat global completed, sans effacer les effets constatés. |
| 13. Recovery et annulation non branchés | **Corrigé dans le parcours d'actions** : opérations interrompues signalées, aucun rejeu, Ctrl+C/Ctrl+Break pour annuler l'exécution approuvée. I01 reste plus large. |
| 14. Statut Ok trop optimiste | **Corrigé** : exécution, couverture et appui diagnostique indépendants ; domaines non inspectés `not_collected`, statut agrégé partial si nécessaire. |
| 15. Documentation en retard | **Mise à jour avec les lots intégrés** ; ce document remplace les priorités devenues obsolètes. Registre partagé des capacités : Q05. |
| 16. Vérification identique pour toute action | **Corrigé** : aucun verdict de performance après ouverture d'une fenêtre ; bilan stockage pour cleanup ; comparaison système pour fermeture normale. |
| 17. CI et corpus | **CI corrigée** : validation du commit sans réécriture ni push automatique. Corpus produit et replay restent Q02. |

Autres corrections conservées : ouvertures relatives sans suivi des liens pour empêcher la sortie du scope, contrôle des fenêtres/références dupliquées avant comparaison, abstention locale plutôt qu'un consommateur mémoire insuffisamment étayé, nettoyage des caractères de contrôle dans les textes de terminal, helpers de tests Windows avec `CREATE_NO_WINDOW`.

**Validation historique, à ne pas confondre avec cette réécriture :** le lot `92b7a28` avait 214 tests Windows et 203 Linux/WSL réussis ; les validations de la [PR #15](https://github.com/nassim-arifette/pc-agent/pull/15) concernent le contexte stockage ajouté ensuite. Le test live fournisseur reste séparé des réponses scriptées. La présente modification documentaire n'annonce aucun nouveau résultat de benchmark ni recette native. La [checklist](validation.md) reste la référence des vérifications encore requises.

<a id="sources-recherche"></a>

## 8. Sources et limites de la recherche

Recherche initiale et réévaluation du **27 septembre 2026**. Les notes antérieures sont conservées et les sources du nouveau classement MFT/USN et des parcours fichiers ont été vérifiées. Les pages évolutives décrivent l'état consulté, pas une disponibilité sur tout PC. Les performances des éditeurs ne sont pas celles de PC Agent. Les liens techniques ci-dessous sont des sources primaires.

### Repères produit conservés

| Réf. | Source et observation | Conséquence proposée |
|---|---|---|
| **RP01** | [Microsoft PC Manager](https://pcmanager.microsoft.com/en-us) : gestion de stockage et raccourcis Windows. | Guidage et effet expliqué, sans reprendre les promesses commerciales de « boost ». |
| **RP02** | [Storage Sense](https://support.microsoft.com/en-us/windows/experience/storage-filemanagement/manage-drive-space-with-storage-sense) : réglages distincts pour temporaires, corbeille, téléchargements et cloud. | Séparer effets, périmètres et autorisations ; retrait de copie locale distinct de suppression. |
| **RP03** | [RAMMap](https://learn.microsoft.com/en-us/sysinternals/downloads/rammap), publication affichée du 26 mars 2026 : catégories mémoire et snapshots. | Expliquer mémoire attribuée/non attribuée et comparer des observations datées. |
| **RP04** | [System Informer](https://system-informer.com/) : graphes et lien historique vers un processus, même terminé. | Conserver identités/contexte pendant la capture ; corrélation temporelle distincte de causalité. |
| **RP05** | WizTree : [MFT](https://diskanalyzer.com/about), [administration et écarts de tailles](https://diskanalyzer.com/faq). | Inventaire rapide en **A**, navigation et comptabilité ensemble ; module de lecture élevé contrôlé. Aucun chiffre concurrent repris comme benchmark local. |
| **RP06** | [WinDirStat](https://windirstat.net/) : scan NTFS direct lorsque disponible, navigation, tailles, doublons par contenu et exports. | Rapidité et exploration sont des attentes établies ; ne pas fonder la comparaison sur une ancienne version uniquement classique. |
| **RP07** | [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) : capteurs multiples, certaines lectures administrateur et différences selon le matériel. | Étudier un adaptateur ciblé avec validation/coût/licences ; aucune couverture universelle supposée. |
| **RP08** | [HWiNFO](https://www.hwinfo.com/about-software/) : inventaire, capteurs, journaux et exports. | Rendre session et faits explorables ; l'existence de mesures ne valide pas une prédiction de panne. |
| **RP09** | [Windows Performance Recorder](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/introduction-to-wpr) : profils, fichiers ou buffers circulaires, détail et surcharge variables. | Capture légère d'abord, marqueur du symptôme, traces spécialisées ensuite ; coût mesuré. |
| **RP10** | [Process Monitor](https://learn.microsoft.com/en-us/sysinternals/downloads/procmon) : activité fichiers/registre/processus et traces détaillées. | Export avec expurgation réelle ; filtrer l'affichage ne retire pas forcément les données sensibles de la trace. |
| **RP11** | [PresentMon](https://github.com/GameTechDev/PresentMon) : mesure des performances graphiques par image et limites documentées. | Mesurer directement la fluidité dans un parcours spécialisé, sans la déduire des pourcentages CPU/GPU. |
| **RP15** | [Agent des paramètres Windows](https://support.microsoft.com/en-us/windows/experience/exploring-windows-settings) sur les PC compatibles. | L'assistance aux réglages existe déjà ; éprouver la valeur de l'investigation et de la vérification de PC Agent. |
| **RP16** | [ETW MCP Early Preview](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/etw-mcp-early-preview-july-2026). | Complément expert à surveiller, sans en faire une dépendance du parcours simple ou une validation du raisonnement modèle. |

### Références ajoutées pour les priorités et les fichiers

| Réf. | Source et fait utile | Application à la feuille de route |
|---|---|---|
| **RP17** | [FAQ Everything](https://www.voidtools.com/faq/) : index NTFS actualisé par USN, accès via service ou administration. | Distinguer scan initial et rafraîchissement ; ne pas copier l'obligation de service permanent dans PC Agent. |
| **RP18** | Microsoft : [Change Journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals), [identifiant](https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier), [curseur et entrées expirées](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v0). | Index local récupérable, détection des trous et reconstruction ; le journal d'actions applicatif n'est pas USN. |
| **RP19** | Microsoft : [FSCTL_ENUM_USN_DATA](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data), [USN_RECORD_V2](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2). | Énumérer les identités/noms ne suffit pas à toute la comptabilité ; mesurer l'enrichissement et valider les versions/longueurs. |
| **RP20** | [Czkawka/Krokiet](https://github.com/qarmin/czkawka) : cœur Rust, doublons, similarité et fichiers invalides, avec cache. | Repère d'architecture et de parcours ; pas de reprise de code sans étude de licence ni de confiance aveugle dans une détection. |
| **RP21** | Defender : [Get-MpComputerStatus](https://learn.microsoft.com/en-us/powershell/module/defender/get-mpcomputerstatus), [Start-MpScan](https://learn.microsoft.com/en-us/powershell/module/defender/start-mpscan). | État/scan via outil compétent, disponibilité et effets à vérifier ; aucune commande libre ni antivirus maison. |
| **RP22** | Microsoft : [sfc](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/sfc) vise les fichiers système protégés. | Ne pas présenter ce contrôle comme validation universelle des documents/photos/archives ; parcours d'intégrité séparés. |
| **RP23** | [Known Folders](https://learn.microsoft.com/en-us/windows/win32/shell/known-folders). | Résoudre les dossiers Windows et leurs redirections ; les variables d'environnement ne constituent pas une cartographie complète. |
| **RP24** | [Espace WSL](https://learn.microsoft.com/en-us/windows/wsl/disk-space), [portée de Docker prune](https://docs.docker.com/reference/cli/docker/system/prune/). | Expliquer VHD, stockage invité et catégories Docker avant toute proposition ; pas de nettoyage global implicite. |
| **RP25** | [Powercfg](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/powercfg-command-line-options), [SleepStudy](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/modern-standby-sleepstudy). | Références de la recherche initiale pour l'alimentation/veille ; compatibilité Modern Standby et privilèges à vérifier pour le parcours choisi. |
| **RP26** | [CredWriteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew). | Piste de la recherche initiale pour le coffre Windows ; intégration, retrait du secret et comportement sur erreur restent à valider. |

### Témoignages qualitatifs conservés

Ces billets ont servi à formuler des hypothèses utilisateur lors de la recherche initiale. **Ils ne constituent ni une référence technique, ni une mesure de fréquence, ni une preuve de cause.** RP12 et RP14 avaient été accessibles via le contenu indexé du moteur, avec échec de l'ouverture directe ; ils ne sont pas présentés comme relus intégralement pendant cette réévaluation.

| Réf. | Témoignage | Hypothèse à tester |
|---|---|---|
| **RP12** | [Saccade audio/vidéo intermittente](https://www.reddit.com/r/techsupport/comments/1gb5xyp), r/techsupport, 24 octobre 2024. | Besoin de capture et de marqueur, avec rétention et limites expliquées — D01/I03. |
| **RP13** | [Mémoire totale incomprise](https://www.reddit.com/r/WindowsHelp/comments/1rh2glj/memory_usage_not_adding_up_which_causes/), r/WindowsHelp, 28 février 2026. | Expliquer les métriques et la part non attribuée avant de recommander une intervention — D04/D06. |
| **RP14** | [Perte de fichier attribuée au nettoyage de Téléchargements](https://www.reddit.com/r/datarecovery/comments/1ifisy4), r/datarecovery, 1er février 2025. | Protéger les fichiers personnels et rendre exclusions/effets compréhensibles — S01/S06/U03 ; la cause alléguée n'est pas établie. |

Cette recherche éclaire les choix ; la validation finale reste l'usage sur des PC réels, les corpus reproductibles et les résultats mesurés.
