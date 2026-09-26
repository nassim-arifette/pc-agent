# PC Agent — Spécification fonctionnelle et technique

**Version :** 0.1 — 25 septembre 2026  
**Nom :** PC Agent est un nom de travail, pas une marque validée.  
**Statut :** spécification de conception ; aucune fonctionnalité, mesure de performance ou validation décrite ici ne doit être présentée comme déjà réalisée.  
**Cible initiale :** Windows 11 x64, utilisateurs individuels.  
**Track proposé :** Best Apps and Agents, Nebius × NVIDIA Global AI Hackathon.

## 0. Lecture et décisions de cadrage

**P0** désigne une exigence indispensable à la version soumise. **P1** désigne une extension souhaitée pour le hackathon, mais qui ne doit pas fragiliser P0. **P2** désigne une évolution après le hackathon. « DOIT » signifie obligatoire ; « DEVRAIT » signifie recommandé sous réserve de validation technique.

Les exemples de dialogues sont illustratifs. Les budgets chiffrés sont des objectifs de conception à mesurer, et non des résultats. Les sources officielles sont référencées par `[Sxx]`, avec leurs adresses à la fin du document.

Décisions structurantes : construire un **agent d'investigation**, pas un détecteur universel d'anomalies ; conserver les collecteurs, l'historique et les permissions sur le PC ; utiliser un modèle NVIDIA Nemotron sur Nebius pour conduire l'enquête ; proposer des actions bornées avec consentement ; accepter « aucune intervention utile identifiée » comme résultat réussi.

## 1. Vision produit

### 1.1 Problème

L'utilisateur veut comprendre pourquoi son PC ralentit, ce qui consomme sa mémoire ou son stockage, ce qu'il peut modifier sans perdre des données importantes, et si son matériel suffit réellement à son usage.

Le produit doit transformer des mesures techniques en décisions compréhensibles, vérifiables et proportionnées. Il ne doit pas promettre une accélération systématique, inventer une panne ni pousser à un achat pour produire une réponse.

### 1.2 Proposition de valeur

> Un ingénieur système IA pour Windows qui enquête à partir de mesures réelles, explique les problèmes de performances et de stockage, propose des corrections ou une évolution matérielle lorsque les preuves le justifient, puis mesure ce qui a changé.

Pitch de travail pour la soumission :

> An AI system engineer for Windows that investigates real system telemetry, explains performance and storage problems, proposes controlled actions or evidence-backed upgrade advice, and measures what changed afterward.

### 1.3 Public initial

Le public de conception est constitué de développeurs, joueurs et utilisateurs Windows exigeants qui veulent comprendre leur machine sans devenir experts de ses compteurs. La V1 ne couvre pas l'administration d'un parc d'entreprise.

L'objectif produit n'est pas de maximiser le nombre de suppressions, la quantité de RAM « libérée » ou les alertes. Il est de conduire à une **décision utile, étayée et contrôlée par l'utilisateur**.

## 2. Contraintes du hackathon

Exigences vérifiées le 25 septembre 2026 dans le règlement [S01] :

| Sujet | Conséquence pour le projet |
|---|---|
| Technologies | Appel réel à Token Factory, ou exécution sur AI Cloud ; utilisation d'un modèle NVIDIA open source. |
| Soumission | Dépôt public sous licence open source, sources nécessaires, README, build ou démo testable. |
| Présentation | Vidéo de moins de trois minutes et éléments en anglais ou traduits. |
| Échéance | 30 octobre 2026, 10 h PDT, soit 18 h à Paris. |
| Évaluation | Accès gratuit au projet jusqu'à la fin du jugement, le 15 décembre 2026. |

**Choix : Best Apps and Agents.** Le parcours multi-outils correspond à l'orientation annoncée par les organisateurs [S02]. La mémoire personnelle reste une capacité du produit, sans imposer le track Personal AI.

## 3. Principes non négociables

**Mesurer avant d'interpréter.** Chaque nombre présenté comme observation doit être rattaché à une collecte, à une unité et à une période. Les valeurs manquantes sont inconnues, jamais remplacées par zéro.

**Séparer faits et hypothèses.** « Ce processus a augmenté de 3 Gio » n'est pas « ce processus a une fuite mémoire ». Une corrélation ne devient pas une cause certaine par reformulation de l'IA.

**Dissocier diagnostic et autorisation.** Le modèle peut demander une inspection ou proposer un plan. Il ne dispose pas de l'autorité nécessaire pour exécuter une modification.

**Ne pas forcer une solution.** Les conclusions autorisées incluent : correction logicielle probable, configuration à examiner, limite matérielle probable, fonctionnement cohérent, ou informations insuffisantes.

**Préserver la confiance.** Pas de score de santé alarmiste, de bouton « booster », de suppression qualifiée de « sans risque », de gain de FPS non mesuré, ni d'affirmation d'anonymat fondée sur le seul retrait du nom d'utilisateur.

**Limiter son propre coût.** L'app doit mesurer son empreinte, limiter les scans et les appels cloud, et rester utilisable sans analyse IA.

## 4. Périmètre de livraison

| Domaine | P0 — socle soumis | P1 — extension hackathon | P2 — après hackathon |
|---|---|---|---|
| Diagnostic | CPU, mémoire, processus, E/S de base, stockage ; preuves et limites | Analyse approfondie d'événements, traces ciblées | Diagnostics spécialisés supplémentaires |
| Stockage | Scan progressif fiable ; gros postes ; propositions de nettoyage | Accélération NTFS/MFT-USN validée ; rafraîchissement incrémental | Optimisations supplémentaires et autres systèmes de fichiers |
| Actions | Plans détaillés, consentement local, catalogue restreint, vérification | Adaptateurs de nettoyage et de démarrage supplémentaires | Automatisations explicitement déléguées |
| Matériel | Inventaire ; avis RAM/stockage conditionnel ou abstention | Informations GPU/VRAM enrichies | Compatibilité détaillée, autres mises à niveau |
| Mémoire personnelle | Historique de session ; préférences explicites ; persistance facultative | Comparaison de sessions réellement documentées | Références statistiques contextuelles, alertes proactives |
| IA | Nemotron sur Nebius, boucle multi-outils et validation | Recherche documentaire Tavily | Optimisation du routage et du coût |
| Distribution | CLI Windows testable d'abord ; build de soumission réutilisant le même Core Rust ; mode démonstration identifié, documentation | GUI Tauri et installateur amélioré | Signature, mises à jour sécurisées, exploitation commerciale |

Le scanner rapide MFT est une **ambition technique importante**, pas une condition cachée de fonctionnement. Son backend doit pouvoir être remplacé par le parcours classique sans supprimer les fonctions de diagnostic et de recommandation. Aucun résultat de vitesse ne sera annoncé avant mesure.

Sont hors P0 : antivirus, réparation du registre, désactivation de protections Windows, gestion de pilotes, overclocking, modification du BIOS, nettoyage automatique universel, arrêt forcé arbitraire, shell administrateur piloté par le modèle, prise en charge macOS/Linux et gestion distante de plusieurs PC.

## 5. Parcours utilisateur

### U1 — « Pourquoi mon PC rame ? »

L'utilisateur peut préciser ce qu'il faisait et depuis quand il constate le problème. L'app collecte un aperçu, approfondit les ressources pertinentes, recherche les changements observés et présente ses hypothèses avec les preuves et les limites.

Une sortie acceptable serait : « La mémoire disponible est faible pendant l'observation. Ce groupe de processus est le plus gros contributeur identifié. Une contention mémoire est plausible, mais ces mesures ne prouvent pas encore que c'est la seule cause de la lenteur. »

**Acceptation :** une période d'observation est visible ; les ressources non mesurées ne sont pas exclues comme causes ; l'utilisateur peut consulter les mesures et demander une nouvelle observation.

### U2 — « Qu'est-ce qui prend toute ma RAM ? »

L'app présente les processus et groupes applicatifs, sépare les métriques mémoire, explique les variations mesurées et indique les portions qu'elle ne peut pas attribuer proprement.

Une croissance sous charge ne déclenche pas automatiquement une accusation de fuite. Une application exclue par l'utilisateur peut rester visible comme contributrice, mais ne doit pas être proposée à la fermeture.

**Acceptation :** l'app identifie correctement les unités et les instances de processus ; elle ne promet pas que fermer un programme libérera exactement sa mémoire engagée.

### U3 — « Trouve-moi 50 Go à récupérer »

L'app demande ou utilise un volume autorisé, effectue un scan, propose des candidats et construit un plan selon les exclusions de l'utilisateur. Elle distingue données à examiner, contenus régénérables, suppression définitive, déplacement et simple mise à la corbeille.

Si l'objectif n'est pas atteignable, elle l'indique. Elle ne doit pas sélectionner des documents personnels ou des fichiers système pour compléter artificiellement le total.

**Acceptation :** la taille logique, l'allocation observée et l'estimation de récupération ne sont pas confondues ; la liste exacte des cibles est consultable avant toute opération.

### U4 — « Je dois optimiser ou acheter de la RAM ? »

L'app examine la capacité installée, la pression observée, les usages que l'utilisateur souhaite conserver, les configurations connues et la répétition éventuelle du problème. Elle considère d'abord les alternatives compatibles avec ces usages.

Une conclusion possible est : « Une capacité supérieure mérite d'être étudiée pour cette charge ; la compatibilité et le gain exact restent à vérifier. » Une autre conclusion valide est : « L'historique actuel ne suffit pas pour recommander un achat. »

**Acceptation :** aucune capacité exacte n'est déduite automatiquement du seul commit ; pas de lien marchand ou de promesse de gain dans P0.

### U5 — Investigation documentaire [P1]

À partir d'un composant, d'une version ou d'un événement réellement identifié, l'agent peut rechercher une documentation officielle. Il doit différencier un problème documenté applicable de symptômes seulement ressemblants.

## 6. Expérience et interface

### 6.1 Premier lancement

L'app démarre sans privilèges administrateur permanents. L'onboarding explique séparément : collecte locale, éventuel historique persistant, analyse cloud, recherche web et opérations pouvant demander une élévation ponctuelle.

L'utilisateur choisit les volumes ou dossiers inspectables, les exclusions, les applications à préserver et les autorisations d'envoi. Il peut utiliser le tableau de bord sans clé cloud. Le démarrage automatique et la collecte permanente nécessitent un choix explicite.

Une requête « analyse tout mon PC » n'autorise pas à lire le contenu de tous les fichiers ni à envoyer l'inventaire complet à un tiers.

### 6.2 Écrans

**Vue d'ensemble :** ressources actuelles, fraîcheur, couverture, statut de connexion et accès aux quatre questions principales. Aucun indicateur « tout va bien » lorsque des mesures nécessaires manquent.

**Investigation :** conversation d'un côté, preuves et timeline de l'autre. Les étapes affichées correspondent à des outils réellement exécutés, pas à une animation préenregistrée présentée comme enquête live.

**Stockage :** navigation hiérarchique, tailles, catégories, exclusions, état partiel ou complet et plan de récupération. Les détails peuvent être chargés progressivement.

**Actions et résultats :** raison, cibles, effet attendu, conséquences, permissions, récupération éventuelle et résultats réels. Les opérations irréversibles sont visuellement distinctes.

**Historique et réglages :** sessions, consentements, préférences, conservation, budget, gestion des clés et suppression de l'historique.

### 6.3 Conversation et contrôle

Le contexte d'une question de suivi doit référencer les mêmes preuves ou annoncer qu'une nouvelle mesure est nécessaire. « Et Chrome ? », « garde Docker », « seulement sur D: » et « je ne veux rien supprimer » doivent modifier le périmètre sans ambiguïté.

L'utilisateur peut interrompre une enquête, refuser un appel web, ignorer une proposition ou protéger une cible. L'arrêt de la conversation annule les opérations d'inspection en attente, sans prétendre annuler des suppressions déjà terminées.

### 6.4 Stratégie CLI-first

La première interface fonctionnelle est un **CLI Windows**, avant le GUI Tauri. L'objectif est de valider le cœur du produit — collecte, contrats de données, orchestration Nemotron/Nebius, recommandations et garde-fous — sans coupler ces fonctions à une interface graphique. Le GUI reste l'interface cible pour la soumission finale si le calendrier le permet, mais ne doit pas bloquer la validation technique.

Le CLI ne contient quasiment aucune logique métier. Il appelle les mêmes crates Rust et les mêmes contrats que la future interface Tauri. Une fonctionnalité terminée en CLI doit donc être réutilisable dans le GUI sans réécrire le diagnostic, les règles de sécurité ou les actions.

Architecture d'interface visée :

```text
                 ┌─────────────┐
                 │  CLI Windows │
                 └──────┬──────┘
                        │
                        ▼
                ┌───────────────┐
                │   Core Rust   │
                │ telemetry     │
                │ diagnostics   │
                │ storage       │
                │ agent         │
                │ policy/actions│
                └───────┬───────┘
                        │
              ┌─────────┴─────────┐
              ▼                   ▼
        Nebius/Nemotron       Windows APIs

                 ┌─────────────┐
                 │ Tauri / GUI │
                 └──────┬──────┘
                        │
                        └── appelle le même Core Rust
```

#### Commandes CLI prévues

Le nom `pcagent` est provisoire. Les commandes initiales sont :

```text
pcagent snapshot
pcagent processes
pcagent storage scan [PATH|VOLUME]
pcagent storage suggest --target 50GB [--scope PATH]
pcagent diagnose "Pourquoi mon PC rame ?"
pcagent diagnose "Est-ce que j'ai besoin de plus de RAM ?"
pcagent history
pcagent doctor
```

**`snapshot`** collecte et affiche un état système borné : mémoire, CPU, engagement, volumes, E/S disponibles et principaux processus. Il doit fonctionner sans cloud.

**`processes`** affiche les principaux contributeurs et, lorsque possible, leur arbre, leur identité stable pendant la session et les métriques mémoire correctement nommées.

**`storage scan`** lance le scanner progressif. Le backend classique est disponible avant l'accélération NTFS/MFT.

**`storage suggest`** construit un plan de récupération compatible avec les exclusions et le niveau de risque autorisé ; la commande ne supprime rien par défaut.

**`diagnose`** lance la boucle agentique Nemotron/Nebius. Le modèle choisit parmi les outils autorisés, reçoit leurs observations structurées et produit une conclusion avec preuves et limites.

**`history`** expose les observations persistées lorsque l'utilisateur a activé cette fonction.

**`doctor`** vérifie les prérequis et capacités : version Windows, accès aux collecteurs, volumes/NTFS, permissions, base locale, configuration Nebius, modèle sélectionné et composants optionnels. Il ne doit pas effectuer d'action destructive.

#### Mode d'observation de l'agent

Pour le développement et la démo, `diagnose` expose un mode détaillé :

```text
pcagent diagnose "Pourquoi mon PC rame ?" --verbose
```

Ce mode affiche uniquement les **étapes réellement exécutées** :

```text
→ Collecte de la pression système
→ Pression mémoire élevée pendant la fenêtre observée
→ Inspection des principaux contributeurs mémoire
→ Inspection du processus 18244 et de son arbre
→ Lecture de son historique de session
→ Vérification CPU et E/S disque
→ Construction du diagnostic
```

Il ne révèle pas de raisonnement interne non nécessaire et ne simule pas une investigation qui n'a pas eu lieu. Les appels d'outils, leurs paramètres non sensibles, leur durée et leur statut peuvent être journalisés pour débogage.

#### Première milestone technique

Avant MFT, Tavily, GPU enrichi, historique long terme ou nettoyage automatique, le projet doit faire fonctionner de bout en bout :

```text
Question utilisateur
        ↓
Collecte Windows réelle
        ↓
Nemotron via Nebius
        ↓
1..N appels d'outils réels
        ↓
Diagnostic étayé par les mesures
```

La première commande de référence est :

```text
pcagent diagnose "Pourquoi mon PC rame ?"
```

**Critère d'acceptation CLI-first :** cette commande doit être utilisable sur une machine Windows distincte, produire une réponse correcte même lorsque certaines métriques sont indisponibles, et montrer dans `--verbose` les outils réellement exécutés. Ce chemin vertical est prioritaire sur la construction du tableau de bord graphique.

## 7. Collecte et sémantique des métriques

### 7.1 Inventaire

L'inventaire comprend la version de Windows, la mémoire installée, le nombre de processeurs logiques, les volumes, les GPU détectés et les versions de composants accessibles. Les numéros de série et identifiants de machine n'entrent pas dans le contexte cloud.

Les collecteurs utilisent des interfaces Windows documentées et isolées derrière des adaptateurs : PSAPI/Win32 pour les processus, compteurs de performance pour les séries, CIM/WMI pour l'inventaire peu fréquent. Les versions de bibliothèques et SDK sont fixées lors de l'implémentation.

### 7.2 Mémoire

L'app doit distinguer mémoire physique disponible, mémoire résidente (*working set*), mémoire privée engagée et engagement système (*commit*). Les champs PSAPI exposent des notions différentes ; ils ne peuvent pas être renommés indistinctement « RAM utilisée » [S08].

Le ratio d'engagement utilise la limite d'engagement fournie par Windows. Il ne mesure ni l'occupation physique de la RAM ni, à lui seul, l'activité du fichier d'échange [S06].

Les fautes de page totales ne sont pas assimilées aux fautes matérielles. Même les lectures liées à des hard faults peuvent concerner des exécutables ou fichiers mappés, et ne prouvent pas seules une pénurie de RAM [S07]. Le produit doit nommer correctement le compteur disponible et abaisser la certitude lorsqu'il manque des observations.

Les sommes de working sets de processus ne sont pas présentées comme un total exclusif de la mémoire physique. La mémoire partagée, le système et les zones non attribuées doivent être explicitement traités ou marqués comme non ventilés.

### 7.3 Processus et CPU

L'identité d'une instance comprend au minimum l'identifiant de démarrage de Windows, le PID et l'heure de création. Un PID réutilisé ne doit jamais recevoir l'historique ou l'action d'une ancienne instance.

Les regroupements par application sont présentés avec leur méthode et leurs limites. Le nom `java.exe` ou `chrome.exe` seul n'est pas une identité fiable. L'arbre de processus observé ne constitue pas une autorisation de fermer tous ses enfants.

La convention de pourcentage CPU doit être explicite : fraction de la capacité totale, avec vue par cœur si disponible. Une moyenne globale modérée n'exclut pas une limitation d'un thread ou d'une ressource non instrumentée.

### 7.4 E/S, disque, GPU et démarrage

Les collecteurs distinguent compteurs d'E/S d'un processus et activité physique d'un disque. Les compteurs susceptibles d'inclure des opérations mises en cache ou d'autres types d'E/S doivent être étiquetés comme tels.

L'analyse disque combine, quand elles sont disponibles, latence, débit, occupation et contexte. Aucun seuil de queue universel n'est présenté comme diagnostic valable pour tous les supports.

Le GPU est instrumenté par capacités : informations Windows disponibles, puis NVML pour les matériels et fonctions NVIDIA pris en charge [S17]. Une fonction absente retourne `unsupported`, pas zéro. Le diagnostic principal doit fonctionner sans GPU NVIDIA sur le PC ; l'inférence du modèle se fait chez Nebius dans l'architecture choisie.

L'inventaire de démarrage et de services est en lecture seule dans P0. Un élément configuré au démarrage n'est pas pour autant responsable d'un délai mesuré. L'attribution d'un coût de boot attend une mesure adaptée [P1].

### 7.5 Cadence et qualité

Valeurs initiales de conception : collecte toutes les deux secondes pendant une investigation, cadence allégée de dix à trente secondes en observation consentie, inventaire matériel au lancement puis sur événement pertinent. Ces valeurs doivent être ajustées aux budgets mesurés.

Chaque résultat contient source, unité, fenêtre d'observation, fraîcheur, couverture et statut. Les statuts minimaux sont `ok`, `partial`, `unsupported`, `permission_denied`, `stale`, `cancelled` et `error`. Les erreurs ne sont pas absorbées silencieusement.

## 8. Moteur de diagnostic

### 8.1 Répartition des responsabilités

Le code déterministe calcule les taux, agrégats, variations, contrôles de qualité et indicateurs. Nemotron choisit les inspections utiles, rapproche les éléments et formule une explication à partir des preuves disponibles.

Les règles ne sont pas un moteur de vérité universelle. Elles génèrent des observations ou des hypothèses candidates, jamais une conclusion certaine fondée sur un seul seuil.

### 8.2 Familles d'hypothèses

**Pression mémoire :** faibles disponibilités persistantes, demandes de mémoire et lectures pertinentes, avec contexte de charge. Le commit proche de sa limite constitue un signal distinct à examiner.

**Croissance d'un processus :** variation réellement observée, durée, changement de charge connu et comparaison de métriques identiques. La formulation privilégiée est « croissance observée » ; « fuite probable » exige des observations supplémentaires.

**Contention CPU ou disque :** saturation durable et correspondance temporelle avec le symptôme, en précisant les compteurs absents et les autres explications possibles.

**Capacité de stockage :** espace disponible, croissance depuis un précédent scan comparable et répartition des allocations. La responsabilité d'une application n'est affirmée que si son association au dossier est établie.

**Limite matérielle :** difficultés répétées sur une charge que l'utilisateur souhaite conserver, après examen des causes logicielles et de configuration observables.

### 8.3 Format d'une conclusion

Une conclusion contient : résumé, faits établis, hypothèses, contre-indices, mesures manquantes, prochaine étape et niveau d'appui des preuves.

Les niveaux sont `étayé`, `plausible` ou `insuffisant`, avec une justification. Ce ne sont pas des probabilités statistiques. Un fait exact et une recommandation incertaine peuvent coexister dans le même rapport.

Un mécanisme de validation doit vérifier les références de preuves et empêcher la présentation de nombres inventés. La validation des références ne prouve pas à elle seule la validité d'une conclusion causale.

### 8.4 Cas explicitement interdits

L'app ne doit pas déduire qu'un processus est malveillant de sa consommation, qu'un GPU est innocent d'un seul pourcentage bas, qu'une grosse allocation est inutile, ou qu'une réduction de mémoire prouve une amélioration générale de performances.

## 9. Scanner de stockage et accélération NTFS

### 9.1 Contrat commun

Deux backends partagent la même interface : parcours classique progressif [P0] et accélération NTFS/MFT-USN [P1]. Le scanner produit un inventaire de métadonnées dans les limites autorisées ; il ne lit pas le contenu des documents pour les classer.

Le parcours classique doit être annulable, borné en concurrence, robuste aux noms Unicode et chemins longs, et ne pas suivre par défaut les liens ou points de réanalyse. Chaque dossier inaccessible apparaît dans les limites du scan.

Les volumes réseau, non pris en charge ou verrouillés ne sont pas analysés comme s'ils étaient accessibles. Le scan ne monte pas de volume ni ne déverrouille BitLocker.

### 9.2 MFT / USN

`FSCTL_ENUM_USN_DATA` permet une énumération liée aux enregistrements MFT [S09]. Mais les structures USN de base n'offrent pas directement toute la comptabilité des tailles et de l'espace récupérable [S10]. Le backend doit donc prévoir un enrichissement des métadonnées plutôt que promettre un inventaire complet à partir de ce seul appel.

Les tailles logiques, allocations et nombres de liens peuvent être obtenus par des interfaces de métadonnées adaptées, notamment les informations standard des fichiers [S11]. Le scanner doit mesurer le coût de cet enrichissement dans ses benchmarks.

Le chemin accéléré reste en lecture seule. Pas d'écriture MFT, pas de modification du journal USN pour rendre une démo possible, et pas de parseur raw-volume non validé dans P0. Un accès technique au volume ne doit pas élargir le périmètre de données divulguées au-delà des autorisations de l'utilisateur.

Le suivi incrémental [P1] conserve un curseur de journal et reconnaît les cas de journal remplacé, tronqué ou de données manquées. Dans ces cas, l'index est marqué incomplet et un nouveau scan est proposé ; aucun historique n'est reconstruit fictivement.

### 9.3 Comptabilité

Le modèle de données distingue **taille logique**, **allocation sur le volume**, **estimation de récupération** et **gain observé après opération**. Une estimation inconnue reste inconnue.

Les fichiers sont identifiés par volume et identifiant de fichier lorsque possible. Les liens physiques, points de jonction et autres alias doivent être traités explicitement pour éviter le double comptage [S12]. Les agrégats parent/enfant d'un plan doivent aussi être dédupliqués.

Fichiers creux, compressés, images VHD/VHDX et espaces synchronisés nécessitent des règles spécifiques. Les placeholders cloud ne doivent pas être téléchargés par le scan. Supprimer un élément synchronisé et retirer seulement sa copie locale sont deux opérations différentes ; elles ne sont pas interchangeables.

Une différence entre espace occupé du volume et total attribué aux fichiers reste affichée comme espace non attribué ou couverture incomplète. L'app ne promet pas que la somme des dossiers explique tous les octets d'un volume.

### 9.4 Dates et fréquence d'usage

Une date de modification n'est pas une dernière utilisation. Les dates d'accès ne sont pas une preuve fiable et universelle d'activité ; Windows documente notamment des comportements différés ou variables pour ces horodatages [S13].

« Ce jeu n'a pas été utilisé depuis trois mois » exige une source d'activité réellement disponible et explicitement autorisée. Sans cette source, l'app affiche « dernière utilisation inconnue ».

## 10. Conseiller de nettoyage

### 10.1 Classification

| Classe | Traitement |
|---|---|
| Contenu régénérable identifié | Proposition uniquement si un adaptateur documente son propriétaire, sa fonction, ses conditions et les conséquences de sa suppression. |
| Téléchargement ou ancien installateur indépendant | Taille et métadonnées visibles ; décision explicite de l'utilisateur. |
| Dossier de build, dépendances ou cache de développement | Recréation, coût, disponibilité réseau et risque de contenu manuel examinés ; aucune suppression sur le seul nom du dossier. |
| Logs et dumps | Rôle diagnostique expliqué ; conservation privilégiée lorsqu'une investigation est en cours. |
| Documents, projets, jeux, modèles ML, VM, archives | Inspection ou désinstallation/déplacement guidés ; jamais classés inutiles automatiquement. |
| Système, récupération et caches d'installation nécessaires | Suppression directe interdite ; renvoi vers un mécanisme officiel lorsqu'il existe. |

En particulier, le cache Windows Installer n'est pas un ensemble de fichiers jetables. Microsoft déconseille sa suppression car il est utilisé pour réparer, modifier ou désinstaller des logiciels [S14].

La liste protégée comprend notamment les composants système, `Windows\Installer`, `WinSxS`, les fichiers de pagination, les données de récupération, et les emplacements ambigus. Le dossier `AppData` n'est jamais effacé globalement. Un dossier temporaire n'est pas automatiquement une cible autorisée.

### 10.2 Plan orienté objectif

L'utilisateur peut fixer un objectif de récupération, protéger des catégories, refuser une désinstallation ou imposer des opérations réversibles. Le moteur déterministe produit les ensembles autorisés ; le modèle aide à expliquer les compromis sans inventer des candidats.

Chaque ligne comprend la cible locale, sa catégorie, les octets estimés, la justification, les effets secondaires, les conditions de régénération, la méthode et son statut de récupération. Aucun groupe risqué n'est présélectionné.

Le produit doit pouvoir dire : « Je n'ai identifié qu'une récupération partielle compatible avec tes contraintes. » La capacité à refuser un objectif trop agressif fait partie de la réussite.

### 10.3 Réversibilité et espace réellement libéré

Une mise à la corbeille ou une quarantaine sur le même volume n'est pas comptabilisée comme une libération de sa capacité. Microsoft précise qu'il faut vider la corbeille pour récupérer cet espace [S19]. Le compteur final lit l'espace disponible réel après opération.

Une suppression définitive n'a pas de bouton d'annulation fictif. Un contenu régénérable n'est pas pour autant restaurable à l'identique. Un déplacement vers un autre volume doit vérifier la copie avant toute suppression de la source [P1].

Les sauvegardes, quarantaines et journaux produits par l'opération sont inclus dans le bilan. L'interface ne peut promettre simultanément une conservation locale intégrale et le gain de place correspondant à la disparition de cette même copie.

## 11. Actions contrôlées

### 11.1 Catalogue P0

Le catalogue initial contient : ouvrir une cible dans l'Explorateur, ouvrir une page autorisée des paramètres Windows, demander une fermeture normale d'une application compatible, déplacer vers la corbeille des fichiers précisément sélectionnés et exécuter un nettoyage limité à des adaptateurs explicitement pris en charge.

Chaque adaptateur de nettoyage doit déclarer ses cibles, versions ou conditions reconnues, préconditions, exclusions, mode d'exécution, statut de récupération et tests. Un adaptateur non validé reste en mode recommandation. Le scénario de démonstration peut utiliser un cache synthétique contrôlé, clairement identifié comme tel.

Les arrêts forcés, services système, modifications de pilotes, désactivation de protections et commandes arbitraires sont absents du catalogue P0. Un refus de fermeture normale n'entraîne jamais une escalade silencieuse vers un arrêt forcé.

### 11.2 Préparation et consentement

L'agent produit une intention ; le moteur local prépare un manifeste immuable. L'utilisateur voit les cibles, l'effet attendu, les risques, les permissions et les limites d'annulation.

L'autorisation est générée uniquement par l'interface locale de confiance. Un message de chat disant « fais tout » ou un texte de page web ne vaut pas validation d'un manifeste précis.

Une autorisation est liée à une empreinte du manifeste, à la session et à une durée courte configurable. Le jeton d'autorisation n'est jamais transmis au modèle. Toute modification du plan invalide l'accord précédent.

### 11.3 Revalidation et exécution

Avant chaque action, l'exécuteur vérifie à nouveau l'identité, l'emplacement, les exclusions, les permissions et les préconditions. Un processus redémarré ou un fichier remplacé nécessite un nouveau plan.

Pour les fichiers, l'implémentation doit prévenir les redirections par liens et les changements de cible entre vérification et opération. Si un backend ne peut pas assurer le lien entre la cible validée et la cible opérée, cette opération n'est pas exposée comme exécutable.

L'exécution est journalisée par élément. Elle n'est pas présentée comme une transaction atomique globale : certains fichiers peuvent être traités et d'autres refusés. Après un crash, la reprise inspecte les états connus ; elle ne rejoue pas aveuglément une suppression.

États attendus : `draft → validated → awaiting_approval → approved → executing → verifying → completed | partial | failed | cancelled`. Les cas de plan expiré ou invalidé doivent être visibles.

## 12. Conseils matériels

### 12.1 Décider avant de dimensionner

L'agent distingue correction logicielle, configuration, adaptation de l'usage et éventuel besoin matériel. L'utilisateur doit pouvoir déclarer que Docker, sa VM ou son logiciel de création sont nécessaires : leur fermeture n'est alors pas une amélioration utile de son scénario.

Un avis favorable à plus de RAM doit reposer sur une charge représentative, des manifestations répétées ou soutenues de contention, les limites des corrections identifiées et les exigences de l'utilisateur. Une observation isolée peut seulement déclencher une nouvelle mesure.

Le commit ne permet pas de calculer mécaniquement la RAM à acheter. Les fichiers d'échange répondent à des besoins d'engagement et de diagnostic propres à la machine [S06, S07]. P0 ne modifie pas leur configuration.

### 12.2 Capacité et compatibilité

Les capacités suggérées sont des hypothèses argumentées, pas des garanties de performances. L'app doit distinguer besoin estimé, capacité envisageable et compatibilité confirmée.

Avant toute recommandation précise de module [P2], il faudra vérifier le modèle exact de machine ou carte mère, la mémoire éventuellement soudée, les emplacements, le type DDR, les limites du fabricant et les contraintes documentées. Une donnée firmware absente ne doit pas être remplacée par une supposition.

P0 n'effectue pas d'achat et ne fournit pas d'affiliation. Pour le SSD ou la VRAM, le même principe d'observation et d'abstention s'applique.

## 13. Agent NVIDIA / Nebius

### 13.1 Architecture d'inférence

L'orchestrateur et les outils restent locaux. Le contexte autorisé et minimisé est transmis à un modèle NVIDIA Nemotron via Nebius Token Factory. Le modèle sélectionne les outils suivants ; le programme local valide et exécute les inspections.

Token Factory documente les appels d'outils et précise que leur exécution appartient au client, pas au modèle [S03]. Cette séparation doit être conservée dans l'architecture de sécurité.

Le nom exact du modèle n'est pas inventé dans la spec. La configuration `NEBIUS_MODEL_ID` doit correspondre à un modèle NVIDIA effectivement disponible pour le compte. La découverte du catalogue est documentée par Nebius [S04]. Un test d'intégration réel doit valider ce modèle, l'appel d'outil, la réponse structurée, la latence et les erreurs avant de figer la version soumise.

La validation JSON reste locale même lorsque le fournisseur prend en charge des sorties structurées [S05]. Un changement de modèle nécessite de refaire les tests concernés. Aucun basculement silencieux vers un modèle non admissible n'est autorisé dans la démonstration.

### 13.2 Boucle d'enquête

`Question → aperçu → hypothèse → inspection ciblée → preuves → proposition → décision utilisateur → action autorisée → mesure de contrôle → rapport`.

L'agent ne doit pas appeler tous les outils systématiquement. Les investigations peuvent terminer sans modification, avec une nouvelle question ou avec une demande de mesures supplémentaires.

Bornes initiales : six tours d'inférence, douze appels d'outils, deux recherches web et quatre-vingt-dix secondes d'enquête hors consentement humain ou scan long explicitement lancé. Ces paramètres sont des limites de conception ajustables. Lorsqu'une borne est atteinte, l'app fournit un état partiel fidèle et demande une prolongation explicite.

Aucun appel d'inférence périodique n'est déclenché par une simple collecte de métriques. Les estimations de coût n'utilisent que des tarifs vérifiés et datés ; sinon, l'app affiche seulement l'usage mesuré.

### 13.3 Outils exposés

| Outil | Périmètre |
|---|---|
| `get_system_snapshot` | Vue globale horodatée et capacités disponibles |
| `get_processes` | Liste bornée, tri autorisé, métrique explicite |
| `get_process_details` | Instance identifiée, pas simple PID réutilisable |
| `get_process_tree` | Relations observées et limites |
| `get_metric_history` | Séries réellement enregistrées sur une fenêtre bornée |
| `get_storage_overview` | Occupation, index et couverture d'un volume autorisé |
| `start_storage_scan` | Tâche locale bornée sur un identifiant de périmètre autorisé |
| `get_largest_items` | Lecture paginée de l'index |
| `get_cleanup_candidates` | Candidats produits par les règles locales |
| `prepare_action_plan` | Plan sans effet de bord ni autorisation d'exécution |
| `get_hardware_inventory` | Capacités observées et inconnues |
| `get_user_preferences` | Contraintes explicites pertinentes pour l'enquête |
| `get_startup_items` [P1] | Inventaire enrichi, sans modification |
| `get_gpu_details` [P1] | Métriques disponibles avec leur provenance |
| `search_vendor_docs` [P1] | Recherche Tavily limitée et expurgée |

L'outil `execute_action` n'est pas exposé au modèle : l'exécuteur reçoit son ordre du contrôleur local après consentement. Il n'existe ni `run_shell`, ni `eval`, ni `delete_path` acceptant un chemin arbitraire.

### 13.4 Fonctionnement dégradé

Sans réseau, sans clé ou en cas d'échec du fournisseur, les collectes, l'inventaire et les règles locales restent accessibles. L'app ne présente pas une réponse en cache comme une enquête live. Une erreur d'inférence n'entraîne aucune opération de maintenance automatique.

Les limites, réponses malformées, délais dépassés et annulations sont traités par un nombre borné de tentatives. Les erreurs d'action ne sont jamais rejouées par un simple retry réseau.

## 14. Recherche documentaire Tavily [P1]

Le rôle de Tavily est d'éclairer un diagnostic quand une documentation externe est utile, pas de remplacer les mesures locales. Son API fournit des résultats et des options de restriction de domaines [S15].

La recherche privilégie Microsoft, NVIDIA et l'éditeur ou constructeur concerné. Les requêtes contiennent uniquement des descripteurs nécessaires : logiciel, version, famille de matériel, version de Windows et événement technique expurgé. Aucun chemin personnel, nom de machine, numéro de série, clé ou dump brut n'y figure.

Chaque résultat retenu conserve titre, source, extrait pertinent, date de consultation et version concernée. Une date absente est déclarée absente. Les solutions ne sont pas transférées à une version différente sans examen de leur applicabilité.

Une page web est une donnée non fiable : ses instructions ne peuvent ni changer la politique locale ni autoriser une action. Une commande trouvée sur un forum n'entre pas dans le catalogue d'exécution. Si aucune source pertinente n'est trouvée, le rapport le dit.

## 15. Architecture technique et sécurité

### 15.1 Composants

| Composant | Responsabilité |
|---|---|
| Interface Tauri 2 + React/TypeScript | Conversation, preuves, consentement, visualisation et paramètres |
| Collecteurs Rust | Acquisition native et inventaire |
| Indexeur de stockage Rust | Parcours classique, backend NTFS, agrégats et couverture |
| Moteur de règles Rust | Calculs, hypothèses candidates, exclusions, plans |
| Orchestrateur local | Sessions, contexte minimal, appels cloud, limites et validation |
| Broker d'actions local | Autorisations, vérifications, exécution et journal |
| SQLite | Séries, index, sessions, préférences et audit |
| Adaptateurs Nebius/Tavily | Échanges réseau autorisés et observabilité de l'usage |

Une seule application modulaire est privilégiée pour P0 ; les composants n'impliquent pas autant de services. Un helper élevé peut exister pour un accès ponctuel clairement identifié. Il n'embarque pas de modèle, n'accepte pas de commandes libres et ne devient pas un service administrateur permanent.

Tauri fournit des mécanismes de permissions et de capabilities, à configurer explicitement [S16]. Les commandes IPC personnalisées nécessitent malgré tout leurs propres validations.

### 15.2 Frontières de confiance

Le renderer, les réponses du modèle, les métadonnées de fichiers et les pages web sont traités comme non fiables. Les identifiants de cibles sont résolus côté Rust ; le modèle ne peut pas transformer un texte arbitraire en accès fichier autorisé.

L'IPC local est limité à l'utilisateur et à la session attendus. Aucun port de contrôle système n'est exposé à Internet. Les échanges cloud sont initiés par le client ; leur retour ne contourne jamais les permissions locales.

Les appels Windows potentiellement dangereux sont encapsulés et testables. Le code de collecte ne possède pas par défaut les capacités du code de modification.

### 15.3 Secrets et distribution

Les clés de développement peuvent appartenir à l'utilisateur, conservées via un mécanisme Windows adapté, hors renderer, logs et dépôt. Une clé de service partagée n'est jamais embarquée dans le binaire.

Pour une distribution avec crédits offerts, prévoir un relais d'inférence authentifié et documenté. Ce relais ne reçoit pas d'autorité d'exécution sur les machines. Le dispositif de test des juges ne doit pas leur imposer l'achat de crédits.

L'installateur et la désinstallation doivent documenter les privilèges, le stockage local et la conservation des données. La signature et un canal de mises à jour vérifiées sont nécessaires avant une diffusion commerciale ; aucun mécanisme de mise à jour non authentifié n'est introduit pour accélérer le MVP.

## 16. Confidentialité et mémoire locale

### 16.1 Collecte minimale

Le produit ne collecte pas par défaut le contenu des fichiers, du navigateur, du presse-papiers, des fenêtres ou des communications. Les arguments complets des processus et les logs bruts sont exclus du cloud.

Les noms de logiciels et les métadonnées peuvent eux-mêmes révéler des informations personnelles. Ils sont minimisés selon le besoin : identifiants opaques, chemins relatifs expurgés, agrégats et sélection des seules preuves pertinentes. Le produit dit « données minimisées », pas « données anonymes » sans preuve.

L'utilisateur peut consulter les catégories de données envoyées, suspendre les échanges, effacer l'historique et modifier ses exclusions. Les transmissions Nebius et Tavily sont distinguées. Aucune garantie de non-conservation côté fournisseur n'est annoncée sans conditions contractuelles vérifiées.

### 16.2 Historique

P0 fonctionne immédiatement sans apprentissage préalable. L'historique persistant est facultatif ; les préférences sont déclarées et modifiables. Un journal minimal des actions est conservé pour expliquer les changements et les opérations partielles.

Proposition de rétention initiale à valider : détail récent d'une heure, agrégats d'une minute sur quarante-huit heures, puis agrégats de cinq minutes sur sept jours. Les investigations enregistrées et les traces d'actions disposent d'une politique explicite, modifiable par l'utilisateur.

Les historiques doivent conserver les lacunes, changements de boot, versions et interruptions. Les détails d'un processus ne sont pas extrapolés pendant les périodes où il n'était pas observé.

### 16.3 Personnalisation ultérieure

P1/P2 peut comparer des sessions lorsque leur contexte est effectivement connu : applications nécessaires, charge déclarée, version et mode d'alimentation. Le nombre de sessions et leur couverture sont affichés.

Médianes, percentiles et variations robustes sont des options, pas une promesse de connaître un « normal » universel. Aucun percentile n'est affiché sans échantillon défini. Les alertes utilisent durée minimale, temporisation et possibilités de silence ; un changement de version ou d'usage peut invalider une référence.

## 17. Données et contrats

### 17.1 Entités principales

| Entité | Informations minimales |
|---|---|
| `DeviceProfile` | Capacités, environnement et inconnues, sans identifiant public permanent |
| `MetricSample` | Valeur, unité, source, temps, intervalle et statut |
| `ProcessInstance` | Boot, PID, création, identité locale et observations |
| `StorageSnapshot` | Périmètre, backend, dates, couverture, identités et tailles |
| `Evidence` | Référence stable à une observation ou source externe |
| `Finding` | Faits, hypothèse, niveau d'appui, contre-indices et limites |
| `Recommendation` | Options, conséquences, préconditions et cibles |
| `ActionPlan` | Manifeste, version, empreinte et statut d'approbation |
| `ActionResult` | Résultat par élément et état après opération |
| `Investigation` | Question, outils exécutés, preuves, coût et issue |
| `UserPreference` | Exclusions, usages à préserver et permissions explicites |

Les messages utilisent `schema_version`. Les tailles sont des octets entiers 64 bits ; l'affichage différencie Go décimaux et Gio binaires. Les conversions IPC préservent les entiers qui dépasseraient la précision numérique du client. Le temps UTC sert à l'affichage et les durées monotones aux taux.

### 17.2 Exemple de preuve — données synthétiques

```json
{
  "schema_version": 1,
  "evidence_id": "ev-demo-001",
  "origin": "synthetic_fixture",
  "metric": "process_private_commit_bytes",
  "entity_ref": "proc-demo-001",
  "value": 4294967296,
  "unit": "bytes",
  "source": "fixture:memory_growth",
  "observed_at": "2026-09-25T09:00:00Z",
  "quality": "ok",
  "limitations": ["Not a measurement from the user's computer"]
}
```

### 17.3 Exemple de conclusion — données synthétiques

```json
{
  "schema_version": 1,
  "finding_id": "finding-demo-001",
  "classification": "needs_more_evidence",
  "evidence_refs": ["ev-demo-001"],
  "facts": ["The observed process has 4 GiB of private committed memory."],
  "hypothesis": null,
  "support": "insufficient",
  "missing": ["Physical memory availability", "Representative workload observation"],
  "recommended_next_step": "get_system_snapshot"
}
```

### 17.4 Exemple de plan — données synthétiques

```json
{
  "schema_version": 1,
  "plan_id": "plan-demo-001",
  "state": "awaiting_approval",
  "action_kind": "recycle_selected_files",
  "target_refs": ["file-demo-001"],
  "evidence_refs": [],
  "estimated_immediate_reclaim_bytes": 0,
  "recovery": "recycle_bin_if_operation_supported",
  "preconditions": ["Identity revalidated", "Not excluded", "Not a system or cloud-managed path"],
  "requires_local_approval": true,
  "approval_granted": false
}
```

Ces exemples illustrent les contrats ; ils ne constituent ni du code d'exécution ni des autorisations.

## 18. Vérification avant / après

L'app conserve une fenêtre avant l'action, attend une stabilisation raisonnable et mesure une fenêtre après. Pour le prototype, des fenêtres de soixante secondes constituent un réglage initial, pas une norme scientifique.

Les fenêtres utilisent les mêmes métriques et documentent les changements de charge. La comparaison inclut valeur centrale, dispersion utile, fraîcheur, qualité et opérations exécutées. Un simple instantané favorable n'est pas suffisant pour déclarer un problème résolu.

Les résultats possibles sont : amélioration mesurée du critère, absence d'amélioration nette, régression, comparaison impossible ou besoin d'une nouvelle mesure. Une fermeture nécessaire à la tâche peut améliorer un compteur tout en dégradant l'usage : le rapport doit l'indiquer.

Pour les performances, le rapport ne dit pas « PC 30 % plus rapide » à partir d'une baisse de RAM. Une amélioration de latence ou de FPS exige sa propre mesure représentative. Le produit peut dire : « La pression mémoire mesurée a baissé ; le ressenti de fluidité n'a pas été mesuré. »

Pour le stockage, la variation d'espace libre est mesurée au niveau du volume. Les écritures simultanées, la régénération de caches et les fichiers encore retenus peuvent expliquer un écart avec l'estimation. Le rapport sépare résultat d'opération et variation globale observée.

La comparaison doit être reproductible avec le modèle désactivé : l'IA explique un résultat calculé, elle ne produit pas les nombres.

## 19. Performance, fiabilité et coûts

### 19.1 Objectifs initiaux, à mesurer

Configuration de référence à documenter : Windows 11 x64, 16 Gio de RAM, SSD NTFS, au moins quatre cœurs logiques. Le rapport précise le nombre de processus, de fichiers, l'état du cache et les exclusions.

| Mesure | Objectif initial |
|---|---|
| Collecte en veille consentie | Moins de 1 % de CPU total en moyenne sur dix minutes, hors scan et enquête |
| Application en tray | Au plus 150 Mio de mémoire résidente pour l'ensemble de ses processus, objectif à valider |
| Interface active hors scan | Au plus 300 Mio pour l'ensemble app/WebView/helpers, objectif à valider |
| Rafraîchissement local | Premières métriques en moins de deux secondes lorsque les collecteurs sont prêts |
| Conservation des séries | Budget initial de 256 Mio, rotation bornée |
| Index de fichiers | Budget séparé initial de 1 Gio, avec arrêt propre ou réduction de portée si nécessaire |
| Scan | Streaming, pagination, progression et annulation ; durée publiée seulement après benchmark |
| Inference | Nombre de tours et budget bornés ; latence et tokens consignés par investigation |

Les objectifs excluant une phase doivent l'indiquer. L'app ne doit pas masquer son propre processus dans la vue de consommation.

### 19.2 Dégradation contrôlée

Sur batterie ou sous forte contention, l'app propose une collecte allégée et diffère les scans non demandés. Elle ne ralentit pas le PC pour rechercher en continu des anomalies hypothétiques.

Le modèle de stockage évite de charger simultanément tout l'index et tous les historiques en mémoire. Les enrichissements coûteux sont paresseux, bornés et annulables.

## 20. Tests et critères d'acceptation

### 20.1 Vérité diagnostique

Le jeu de tests doit inclure une forte utilisation mémoire sans contention, un commit élevé sans activité démontrée du fichier d'échange, une croissance normale sous charge, des fautes sur fichiers mappés, une limitation d'un cœur, des métriques absentes et un historique insuffisant.

Dans les cas ambigus, la réponse attendue est une limitation explicite ou une investigation supplémentaire, pas une réparation inventée. L'évaluation doit examiner la justesse des faits, l'appui des conclusions et les abstentions pertinentes.

### 20.2 Stockage

Tester liens physiques, jonctions, dossiers imbriqués, permissions refusées, fichiers modifiés pendant le scan, chemins longs, Unicode, fichiers creux/compressés et placeholders cloud. Vérifier que le scanner ne suit pas une redirection hors périmètre et ne télécharge pas le contenu distant.

Tester la différence entre taille logique, allocation et récupération. Une mise à la corbeille ne doit pas être affichée comme gain immédiat. Un dossier parent et ses enfants ne doivent pas être additionnés deux fois dans un plan.

Tester le backend classique lorsque le backend NTFS est indisponible, refusé ou invalidé. Les compteurs de couverture doivent être cohérents et les valeurs partielles clairement identifiées.

### 20.3 Sécurité et actions

Les tests incluent : absence de consentement, plan expiré, cible remplacée, PID réutilisé, double clic, interruption, reprise après crash, opération partielle, refus d'élévation et tentative de modification d'un dossier protégé.

Injecter des instructions hostiles dans un nom de fichier, un résultat Tavily et une réponse de modèle. Ces données ne doivent provoquer aucun élargissement de permission, aucune exécution de commande et aucune fuite de secret.

Les tests de suppression et d'élévation sont effectués sur un corpus jetable ou volume de test, jamais sur des données personnelles nécessaires. Les tests unitaires portables ne remplacent pas une validation native Windows.

### 20.4 IA et intégrations

Un test réel documenté doit montrer : appel au modèle NVIDIA configuré via Nebius, demande d'outil, validation locale, collecte, retour de résultat, synthèse avec preuve et absence d'action sans accord.

Les tests simulés servent aux erreurs, limites, validations et scénarios reproductibles. Ils doivent être distingués des tests live du fournisseur. Tavily, s'il est livré, doit être exercé par au moins une recherche réelle sans données sensibles.

### 20.5 Définition de « prêt à soumettre »

P0 est prêt quand les quatre parcours produisent des réponses étayées ou une abstention correcte, qu'au moins une boucle réelle proposition/consentement/action/mesure est démontrable, que les contrôles de sécurité passent sur le corpus défini, et que le build fonctionne sur une machine Windows de test distincte.

Le README doit distinguer fonctionnalités implémentées, modes simulés et limites connues. Une fonctionnalité documentée mais non testée n'est pas déclarée validée. La version testable est identifiée par un tag et un commit ; sa configuration de modèle, ses règles et ses contrats sont versionnés pour rendre les résultats reproductibles.

## 21. Démonstration et évaluation produit

Le parcours principal conseillé combine une investigation mémoire avec une proposition contrôlée et une mesure de suivi. Le deuxième parcours montre un objectif de stockage et la capacité à refuser les suppressions inadaptées. Une vue secondaire montre le cas « pas de correction logicielle identifiée / matériel à examiner » sans prétendre qu'un achat a déjà prouvé son efficacité.

Les charges de démonstration sont créées dans un environnement isolé, avec plafonds, arrêt automatique et identification explicite. Une charge mémoire synthétique doit être bornée ; la démo ne doit pas risquer d'immobiliser le PC principal.

Une vidéo doit afficher l'app native en fonctionnement, les preuves, une validation utilisateur et un résultat. Un historique synthétique ou rejoué porte une étiquette visible. Aucun gain réel ne doit être remplacé par un compteur scénarisé.

En évaluation interne, comparer le même corpus avec règles seules, réponse IA sans outils et agent avec outils peut aider à vérifier l'utilité réelle de l'orchestration. Les critères sont qualité des décisions, erreurs factuelles, abstentions, coût, délai et actions correctement bloquées — pas seulement qualité rédactionnelle.

## 22. Plan de réalisation

**Lot A — Chemin vertical CLI-first.** Créer `pcagent`, le collecteur mémoire/processus, `snapshot`, `doctor` et `diagnose`. Obtenir un premier appel Nemotron via Nebius, au moins un outil réel choisi pendant l'enquête, une sortie `--verbose` traçable, une preuve affichée et des tests de données manquantes. Valider tôt l'accès au modèle, le contrat d'API et le fonctionnement sur une deuxième machine Windows. Aucun travail GUI ne doit bloquer ce lot.

**Lot B — Investigation utilisable.** CPU, séries de session, identité stable des processus, hypothèses, exclusions, limites d'enquête et mode hors ligne.

**Lot C — Stockage et recommandations.** Scanner classique, comptabilité, cibles protégées, candidats documentés, objectif de récupération et refus corrects. Prototyper séparément le backend NTFS pour mesurer son apport.

**Lot D — Actions et contrôle.** Manifestes, consentement local, catalogue restreint, journal, gestion des courses, résultats partiels et mesure avant/après. Aucune action destructive n'est activée avant ses tests de sécurité.

**Lot E — Finition hackathon.** Conseils matériels prudents, GUI Tauri branché sur le même Core Rust si le calendrier le permet, installateur/build, intégration Tavily et MFT si validées, test sur autre machine, documentation et vidéo. Le CLI reste une interface officielle de diagnostic et de secours ; le GUI ne duplique pas la logique métier.

En cas de tension de périmètre, retirer d'abord les enrichissements GPU, les alertes proactives, les adaptateurs supplémentaires et l'accélération NTFS instable. Ne jamais retirer les protections, la provenance des métriques ou la boucle réelle d'inférence pour conserver une fonctionnalité visible.

## 23. Possibilité de commercialisation

La V1 doit préserver une architecture exploitable commercialement : modules séparés, coûts observables, absence de clés distribuées, permissions explicites, index local, exports et suppression des données.

Le dépôt du hackathon doit respecter l'obligation de licence mentionnée plus haut. L'open source autorise la vente, mais vente et exclusivité propriétaire sont deux questions différentes [S18]. Le choix de licence et la compatibilité des dépendances doivent être tranchés avant publication ; ne pas compter sur un simple retour en privé pour effacer les droits déjà accordés.

Les pistes ultérieures sont des hypothèses à valider : version individuelle avec service d'inférence, support, historique avancé ou offre de diagnostic de parc. Aucun prix, conversion attendue ou marché acquis n'est fixé par cette spec.

Avant une version payante : revue de sécurité, fiabilité des actions, signature et mises à jour vérifiées, traitement des données conforme aux engagements affichés, support des erreurs réelles et tests sur plusieurs configurations. Les fonctions sensibles ne doivent pas devenir agressives pour justifier un abonnement.

## 24. Décisions restant à valider pendant l'implémentation

| Décision | Méthode de résolution |
|---|---|
| Modèle NVIDIA exact | Catalogue du compte Nebius, test live des outils, qualité, latence et coût |
| Licence | Objectif de distribution, dépendances et obligations du règlement |
| Gain réel du backend NTFS | Prototype et benchmark comparatif sur corpus défini |
| Métrologie mémoire détaillée | Capteurs Windows disponibles, coût et qualité de leur attribution |
| Premier adaptateur de nettoyage réel | Documentation, périmètre étroit et tests de sûreté |
| Distribution du crédit de démonstration | Accès fonctionnel pour testeurs sans secret dans le binaire |
| Empreinte tray et UI | Mesure de tous les processus, pas seulement du collecteur Rust |

Aucune de ces décisions ne justifie d'inventer une capacité ou d'affaiblir une règle de sécurité.

## Annexe A — Organisation proposée du dépôt

```text
apps/cli/                 interface CLI Windows ; aucune logique métier lourde
apps/desktop/             interface Tauri/React branchée sur le même Core Rust
crates/telemetry/         collecteurs Windows
crates/storage/           scan classique, index et backend NTFS
crates/diagnostics/       calculs et hypothèses candidates
crates/policy/            exclusions, permissions et validations
crates/actions/           manifestes, adaptateurs et journal
crates/agent/             orchestration locale et adaptateurs cloud
crates/shared/            contrats et types versionnés
services/inference-gateway/  facultatif, crédits de distribution
schemas/                  contrats JSON et validations
fixtures/                 corpus synthétiques, sans données privées
tests/                    tests intégration, sécurité et scénarios
docs/                     architecture, sûreté, benchmark et limites
README.md
LICENSE
```

Les décisions d'architecture importantes doivent être courtes et traçables. Le dépôt ne contient pas de télémétrie personnelle, de journaux privés ou de clés.

## Annexe B — Références officielles

Sources consultées le 25 septembre 2026. Elles documentent les contraintes et interfaces ; elles ne prouvent ni l'implémentation ni les performances de PC Agent.

- **[S01] Devpost — règlement du Nebius × NVIDIA Global AI Hackathon.** https://nebiusglobalaihackathon.devpost.com/rules
- **[S02] Devpost — Here's how judging works.** https://nebiusglobalaihackathon.devpost.com/updates/46204-here-s-how-judging-works
- **[S03] Nebius — Function calling & Tools.** https://docs.tokenfactory.nebius.com/ai-models-inference/function-calling
- **[S04] Nebius — List models.** https://docs.tokenfactory.nebius.com/api-reference/models/list-models
- **[S05] Nebius — Structured output & JSON.** https://docs.tokenfactory.nebius.com/ai-models-inference/json
- **[S06] Microsoft — Introduction to page files.** https://learn.microsoft.com/en-us/troubleshoot/windows-client/performance/introduction-to-the-page-file
- **[S07] Microsoft — Page file sizing and performance counters.** https://learn.microsoft.com/en-us/troubleshoot/windows-client/performance/how-to-determine-the-appropriate-page-file-size-for-64-bit-versions-of-windows
- **[S08] Microsoft — PROCESS_MEMORY_COUNTERS_EX.** https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex
- **[S09] Microsoft — FSCTL_ENUM_USN_DATA.** https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data
- **[S10] Microsoft — USN_RECORD_V2.** https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2
- **[S11] Microsoft — FILE_STANDARD_INFO.** https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_standard_info
- **[S12] Microsoft — Hard Links and Junctions.** https://learn.microsoft.com/en-us/windows/win32/fileio/hard-links-and-junctions
- **[S13] Microsoft — GetFileTime.** https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfiletime
- **[S14] Microsoft — Missing Windows Installer cache.** https://learn.microsoft.com/en-us/troubleshoot/windows-client/application-management/missing-windows-installer-cache
- **[S15] Tavily — Search API.** https://docs.tavily.com/documentation/api-reference/endpoint/search
- **[S16] Tauri — Capabilities.** https://v2.tauri.app/security/capabilities/
- **[S17] NVIDIA — NVML API Reference.** https://docs.nvidia.com/deploy/nvml-api/latest/index.html
- **[S18] Open Source Initiative — Frequently Answered Questions.** https://opensource.org/faq
- **[S19] Microsoft — Free up drive space in Windows.** https://support.microsoft.com/en-us/windows/experience/storage-filemanagement/free-up-drive-space-in-windows

---

**Critère directeur :** le logiciel doit aider l'utilisateur à prendre une bonne décision à partir de faits, pas trouver à tout prix quelque chose à supprimer, arrêter ou acheter.