# Mentor — ton coach WoW

Un coach vocal pour World of Warcraft (pensé pour WoW Forever). Tu maintiens une touche, tu poses ta question à voix haute, il te répond dans le casque et tout s'affiche dans un chat : ce que tu as dit, sa réponse, les macros et les plans.

## Pour tes amis : installer

1. Lancer `Mentor_x.y.z_x64-setup.exe` (pas besoin d'être administrateur).
2. Au premier lancement, l'assistant guide les 4 étapes :
   - **Compte Claude** : installe Claude Code, puis au choix :
     - **Abonnement Claude Pro ou Max** : connexion claude.ai, rien à payer en plus ;
     - **Clé API** (sans abonnement) : à créer sur console.anthropic.com, facturée à l'usage (quelques centimes par question, recherches web en plus). Pense à fixer une limite de dépenses sur la console. La clé est chiffrée avec le compte Windows.

     Le plan gratuit de claude.ai ne suffit pas : il n'inclut pas Claude Code. Chacun utilise son propre compte, on ne partage pas.
   - **Voix et reconnaissance vocale** : téléchargement unique (≈ 610 Mo en mode « Précis », ≈ 250 Mo en « Léger »). Ensuite tout tourne en local.
   - **Touche pour parler** : un bouton latéral de souris est idéal (choisis-en un que le jeu n'utilise pas).
   - **Addon Mentor** : installé en un clic dans `_forever_\Interface\AddOns`.
3. En jeu : `/reload` (ou relancer le jeu), puis `/mentor sync` pour envoyer ta fiche au coach.

Windows 10/11 64 bits. Le coach marche avec n'importe quelle carte graphique (Vulkan) et se rabat sur le processeur si besoin.

## Utilisation

| Action | Comment |
|---|---|
| Poser une question | Maintenir la touche « Parler », parler, relâcher. Ou l'écrire dans le chat. |
| Montrer l'écran | Automatique si ta question parle de l'écran (« regarde », « c'est quoi cet objet »…). Sinon le bouton appareil photo, ou la touche « Parler + capture ». |
| Couper la voix | Touche « Couper », le bouton carré, ou reparler. |
| Oublier la conversation | « Nouvelle session ». |
| Ton personnage | Icône bonhomme en haut. Le coach reçoit la fiche (classe, talents, équipement avec infobulles, sacs, sorts, macros, quêtes) avec ta question suivante dès qu'elle change. |

### Base de connaissances officielle

WoW Forever évolue tout le temps, et Claude ne le connaît pas de mémoire. Mentor garde donc sur le PC une copie des documents officiels de Blizzard et le coach les consulte **avant** de chercher sur le web :

- les **notes de correctifs** (page officielle, en anglais et dans sa traduction française) ;
- les **messages de l'équipe Blizzard sur le forum officiel** : équilibrage des classes, problèmes connus, calendrier, notes de développement de la bêta ;
- les **guides et articles officiels** (nouveautés, donjons, raids, système d'héritage, ensembles de règles…).

La synchronisation est automatique : au démarrage puis toutes les deux heures, en ne téléchargeant que ce qui a changé. Quand Blizzard publie quelque chose, un message apparaît dans le chat avec un bouton « Qu'est-ce qui change pour moi ? ». L'icône livre en haut ouvre la liste des documents (un point doré signale du nouveau) et permet de forcer une mise à jour. Réglages → Jeu pour la désactiver.

Les fichiers sont dans `%APPDATA%\fr.mentor.app\claude\knowledge`. Claude ne peut lire que ce dossier : tout autre fichier du PC lui est refusé. Pour ce que la base ne couvre pas (objets, quêtes, builds), le coach cherche sur le web en privilégiant les pages écrites pour WoW Forever (wowhead.com/forever en premier).

Une conversation laissée plus de 12 heures n'est pas reprise : le coach repart à neuf et reçoit de nouveau ta fiche, ce qui garde les réponses rapides.

### Indicateur par-dessus le jeu

Une petite pastille en haut de l'écran : micro rouge animé selon ta voix quand tu parles, chargement doré pendant que Mentor réfléchit, haut-parleur bleu quand il répond, et un message court si quelque chose cloche. Elle est invisible le reste du temps et les clics la traversent. Réglages → Divers : l'activer ou la désactiver, et choisir sa position. Il faut le jeu en « plein écran fenêtré ».

### Voix

Par défaut, une voix neuronale Microsoft très naturelle (Vivienne ; aussi Rémy, Denise, Henri, Sylvie, Antoine…), qui demande Internet. C'est un service non officiel : s'il ne répond plus, Mentor bascule tout seul sur la voix Piper, installée sur le PC (plus robotique). Réglages → Audio → Voix.

### Builds, objets, macros

- « Lequel de ces deux bâtons je garde ? » : le coach compare avec les infobulles exportées par l'addon.
- « Fais-moi une macro pour… » : la macro apparaît dans une carte avec **Copier** et **Installer en jeu**. Après « Installer en jeu », tape `/reload` : l'addon crée la macro (ouvre `/macro` pour la glisser sur une barre).
- « Fais-moi un plan pour monter niveau 20 » : carte **Plan** avec **Envoyer au suivi en jeu**, puis `/reload` : les étapes s'affichent à l'écran et se cochent au clic.

Les macros, plans et tableaux ne sont jamais lus à voix haute : le coach dit juste « je t'ai mis la macro dans le chat ».

### Commandes de l'addon en jeu

`/mentor` ou son raccourci `/men` (l'addon n'active le raccourci que si aucune commande du jeu ne porte déjà ce nom) :

`/mentor sync` (fiche à jour + reload), `/mentor export` (fiche en texte), `/mentor plan`, `/mentor show`, `/mentor hide`, `/mentor clear`.

## Dépannage

- **Ma fiche personnage n'est pas à jour** : l'addon n'écrit sur le disque qu'au `/reload`, à la déconnexion ou avec `/mentor sync`. Si l'app affiche « Mentor est désactivé dans le jeu », clique sur « Activer l'addon » (jeu fermé) ou coche-le dans le bouton AddOns de l'écran de sélection du personnage.

- **Rien ne se passe quand j'appuie sur la touche** : si WoW ou Battle.net tourne en administrateur, lance aussi Mentor en administrateur.
- **Les captures sont noires** : passe le jeu en « plein écran fenêtré ».
- **Le coach plante au démarrage** : Réglages → Reconnaissance vocale → désactive « Utiliser la carte graphique ».
- **La base de connaissances ne se met pas à jour** : icône livre → « Mettre à jour maintenant » affiche l'erreur. Si Blizzard change la mise en page de son site, les documents déjà téléchargés restent utilisables et le coach se rabat sur le web.
- **Diagnostic complet** : `"%LOCALAPPDATA%\Mentor\mentor.exe" --selftest` écrit un rapport dans `%APPDATA%\fr.mentor.app\selftest.log` (ajoute `--no-claude` pour ne pas interroger Claude). Il synchronise aussi la base de connaissances et pose une question dessus.
- Le journal de l'app : `%APPDATA%\fr.mentor.app\coach.log` (Réglages → Divers → Ouvrir).

## Ce que dit le règlement

- Claude : l'app lance le binaire officiel Claude Code avec **ta** connexion, pour toi seul. Ne la transforme pas en service partagé.
- Jeu : l'addon n'utilise que l'API officielle des addons (lecture de ta fiche, création de macros que tu valides par `/reload`). L'app n'envoie aucune touche au jeu et ne lit pas sa mémoire.

## Compiler (pour toi)

Prérequis (déjà installés sur ton PC) : Rust, Visual Studio Build Tools (C++), CMake, LLVM, Vulkan SDK, Node.js.

```
cd app
npm install
build.cmd build --release      :: binaire seul → C:\cwt\release\mentor.exe
build.cmd test --release       :: tests
build.cmd tauri                :: installeur → C:\cwt\release\bundle\nsis\
```

Pour tester aussi la synchronisation avec les vrais sites de Blizzard : définir `MENTOR_ONLINE_TESTS=1` avant `build.cmd test --release`. Les sources (articles et sujets toujours conservés, catégories du forum) sont en tête de `src-tauri/src/knowledge.rs`.

`build.cmd` charge l'environnement Visual Studio et compile dans `C:\cwt` : les chemins longs cassent la compilation des shaders Vulkan de whisper.cpp. `vendor/whisper-rs-sys` est une copie de la crate avec bindgen 0.73, nécessaire avec clang 23. Toujours compiler en `--release` : en debug, whisper.cpp utilise la bibliothèque C de debug et ouvre des fenêtres d'assertion.
