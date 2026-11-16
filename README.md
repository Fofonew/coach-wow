# Coach WoW

Un coach vocal pour World of Warcraft, pensé pour **WoW Forever**. Tu maintiens une touche, tu poses ta question à voix haute, tu relâches : le coach cherche la réponse et te la dit dans le casque, sans quitter le jeu.

Le cerveau, c'est **Claude**, lancé par la CLI officielle Claude Code avec **ton propre compte**. La reconnaissance vocale tourne en local sur ton PC.

```
touche maintenue → micro → Whisper (local) → Claude (+ recherche web) → voix → casque
```

Le dépôt contient deux versions du même outil :

| | Quoi | Pour qui |
|---|---|---|
| [`app/`](app/) | **Mentor** : l'application Windows (Rust + Tauri 2), avec chat, addon en jeu et installeur | La version à utiliser et à donner à tes amis |
| racine (`coach.py`…) | Le **prototype Python** d'origine, en console | Pour bidouiller, ou pour l'adapter à un autre jeu |

## Mentor, l'application

- **Push-to-talk et chat** : ta question, la réponse, les macros et les plans s'affichent dans une fenêtre. Tu peux aussi écrire ta question.
- **Addon Mentor** : il envoie ta fiche personnage au coach (classe, talents, équipement avec infobulles, sacs, sorts, macros, quêtes) et installe en jeu les macros et les plans que le coach propose.
- **Base de connaissances officielle** : notes de correctifs, messages de l'équipe Blizzard sur le forum et guides officiels, synchronisés toutes les deux heures et consultés avant le web.
- **Captures d'écran** : jointes automatiquement quand ta question parle de ce que tu vois.
- **Indicateur par-dessus le jeu** : une pastille montre quand Mentor écoute, réfléchit ou répond.
- **Voix** : voix neuronale Microsoft (en ligne), avec repli automatique sur Piper (sur le PC).
- **Reconnaissance vocale locale** avec whisper.cpp, sur n'importe quelle carte graphique (Vulkan) ou sur le processeur.

Il faut Windows 10 ou 11 en 64 bits et un compte Claude : abonnement Pro ou Max, ou clé API. Chacun utilise son propre compte.

### Installer

L'installeur `Mentor_x.y.z_x64-setup.exe` est un binaire, il n'est donc pas versionné ici. Il se récupère dans les [Releases](https://github.com/Fofonew/coach-wow/releases) quand une version y est publiée, sinon il se compile (voir plus bas).

1. Lance l'installeur (pas besoin d'être administrateur).
2. Au premier lancement, l'assistant guide les 4 étapes : compte Claude, voix et reconnaissance vocale, touche pour parler, addon.
3. En jeu : `/reload`, puis `/mentor sync` pour envoyer ta fiche au coach.

Le guide complet (utilisation, commandes de l'addon, dépannage) est dans [`app/README.md`](app/README.md).

### Compiler

Prérequis : Rust, Visual Studio Build Tools (C++), CMake, LLVM, Vulkan SDK, Node.js.

```
cd app
npm install
build.cmd build --release      :: binaire seul → C:\cwt\release\mentor.exe
build.cmd test --release       :: tests
build.cmd tauri                :: installeur → C:\cwt\release\bundle\nsis\
```

`build.cmd` charge l'environnement Visual Studio et compile dans `C:\cwt` (les chemins longs cassent la compilation des shaders Vulkan de whisper.cpp). Il suppose les emplacements d'installation par défaut, dont le Vulkan SDK 1.4.363.0 dans `C:\VulkanSDK` : adapte les chemins en tête du fichier si les tiens diffèrent. Compile toujours en `--release`. Les détails sont dans [`app/README.md`](app/README.md#compiler-pour-toi).

## Organisation du dépôt

```
app/                          Mentor, l'application Windows
├── src-tauri/src/            cœur Rust : audio, reconnaissance vocale, voix, Claude, WoW, base de connaissances
├── src-tauri/resources/      addon Mentor (Lua) et prompt du coach
├── src-tauri/dlls/           DLL embarquées dans l'installeur (sherpa-onnx, ONNX Runtime, runtime MSVC)
├── ui/                       interface (HTML, CSS, JavaScript)
├── vendor/whisper-rs-sys/    copie de la crate avec bindgen 0.73, nécessaire avec clang 23
├── tests/                    faux client WoW pour tester l'addon hors du jeu
└── build.cmd                 script de compilation
coach.py                      prototype Python
coach.toml                    ses réglages
coach_prompt.md               son prompt système
requirements.txt              ses dépendances
install.bat, start_coach.bat  installation et lancement du prototype
```

Ce qui reste hors du dépôt (voir `.gitignore`) : l'environnement Python `.venv/`, la voix Piper `voices/` (elle se télécharge, voir plus bas), l'état local `runtime/` et les installeurs générés `dist/`.

## Prototype Python

La première version du coach : un script en console, sans interface ni addon. Il sait aussi passer par ChatGPT, et se règle pour n'importe quel jeu.

Tu maintiens une touche, tu poses ta question à voix haute, tu relâches. Le coach cherche sur le web et te répond dans le casque, sans quitter le jeu.

Le cerveau passe par **tes abonnements**, via les CLI officielles. Aucune clé API, aucun crédit à acheter :

- **Claude** : `claude -p` (Claude Code en mode non interactif), connecté avec ton compte Claude (Pro ou Max).
- **ChatGPT** : `codex exec` (Codex CLI), connecté avec ton compte ChatGPT (plan payant).

Tout le reste tourne en local et gratuitement :

- la reconnaissance vocale avec faster-whisper ;
- la voix avec Piper ;
- les raccourcis avec pynput ;
- les captures d'écran avec mss.

```
touche maintenue → micro → Whisper (local) → claude -p / codex exec (+ recherche web) → phrases → Piper (local) → casque
```

Avec Claude, la réponse est lue **phrase par phrase pendant qu'elle s'écrit**, et le coach annonce ce qu'il vérifie avant de chercher. Tu entends donc quelque chose rapidement, même quand la recherche prend plusieurs secondes. Avec Codex, la réponse est lue d'un bloc une fois terminée.

#### Session persistante (Claude)

- **Un seul processus Claude, gardé allumé.** Il démarre en même temps que le coach, pendant le chargement de Whisper, et reçoit toutes tes questions. Le coach utilise pour cela le mode « streaming input » documenté par l'[Agent SDK](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode) : un processus de longue durée qui garde le contexte d'un tour à l'autre. Il n'y a plus de redémarrage de la CLI à chaque question.
- **La conversation survit au redémarrage du coach.** L'identifiant de session est enregistré dans `runtime/claude/session.json`. Au lancement suivant, le coach reprend la même conversation avec `--resume`.
- **Interruption propre.** Quand tu reparles ou que tu appuies sur Stop, le coach envoie la même demande d'interruption que le SDK officiel, et le processus reste en vie. S'il ne réagit pas dans les `interrupt_grace_seconds`, il est relancé sur la même session.
- **Crash ou session introuvable.** Si le processus plante, il est relancé sur la même session. Si la session n'existe plus côté Claude, le coach en démarre une nouvelle automatiquement.
- **Repartir de zéro.** Seule la touche `new_session` efface la conversation.

### Ce que dit le règlement

- **Claude** : quand tu es connecté avec ton abonnement, `claude -p` consomme les limites de ton abonnement ([article Anthropic](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan)). La connexion OAuth d'un abonnement est réservée à l'usage normal de Claude Code et des applis Anthropic ([conditions Claude Code](https://code.claude.com/docs/en/legal-and-compliance)). Ce script lance le binaire officiel, non modifié, avec ta propre connexion, pour toi seul. Ne le transforme pas en service pour d'autres personnes.
- **Codex** : `codex login` utilise la connexion ChatGPT ([référence CLI](https://learn.chatgpt.com/codex/developer-commands)).
- **Jeu** : le coach ne lit pas la mémoire du jeu et n'envoie aucune touche au jeu. Il se comporte comme un Wowhead qui parle.

### Installation (Windows)

#### 1. Python

Installe Python 3.11 ou plus récent (3.12 conseillé pour la disponibilité des paquets), puis double-clique sur `install.bat`. Ensuite, `start_coach.bat` lance le coach (tu peux lui passer les mêmes options, par exemple `start_coach.bat --verbose`).

À la main, dans le dossier du coach :

```
python -m venv .venv
.venv\Scripts\activate
pip install -r requirements.txt
```

#### 2. Le cerveau (un seul des deux suffit)

**Claude** : installe Claude Code ([guide officiel](https://code.claude.com/docs/en/quickstart)). Lance `claude` une fois et connecte-toi avec ton compte Claude, pas avec la Console. Pour vérifier :

```
claude auth status
```

Le champ `authMethod` doit valoir `claude.ai`. Le script retire `ANTHROPIC_API_KEY` de l'environnement des processus qu'il lance, pour ne jamais basculer sur une facturation API par erreur.

**ChatGPT** : installe Codex CLI, de préférence avec l'installeur natif dans PowerShell :

```
irm https://chatgpt.com/codex/install.ps1 | iex
codex login
codex login status
```

Puis mets `backend = "codex"` dans `coach.toml`.

#### 3. La voix française (Piper)

```
mkdir voices
python -m piper.download_voices fr_FR-siwis-medium --download-dir voices
```

Pour une voix plus naturelle mais en ligne, mets `engine = "edge"` dans la section `[tts]`. Elle utilise les voix neurales de Microsoft Edge via le paquet `edge-tts`, qui n'est pas une API officielle et peut casser du jour au lendemain.

#### 4. Whisper sur la carte graphique (optionnel mais conseillé)

faster-whisper sur GPU demande **cuBLAS pour CUDA 12** et **cuDNN 9**. `requirements.txt` les installe déjà via les paquets pip `nvidia-cublas-cu12` et `nvidia-cudnn-cu12`, et le script les charge tout seul. Si ça ne marche pas, l'autre solution est l'archive de [Purfview/whisper-standalone-win](https://github.com/Purfview/whisper-standalone-win/releases/tag/libs) : décompresse-la et ajoute le dossier au `PATH` ([README faster-whisper](https://github.com/SYSTRAN/faster-whisper)).

Sans ces bibliothèques, mets `device = "cpu"` dans `[stt]`. Si le chargement GPU échoue proprement, le script bascule de lui-même sur le modèle `small` en CPU.

### Tester étape par étape

```
python coach.py --ask "Quels sont les rulesets de WoW Forever ?" --ask "Et lequel pour du RP ?"
```
Ce test vérifie la connexion, la recherche web et la suite de session, sans micro. Les lignes `[speech]` montrent exactement ce qui serait lu à voix haute.

```
python coach.py --say "Bonjour, je suis ton coach."
```
Ce test vérifie la voix et la sortie audio.

```
python coach.py --detect
```
Appuie sur une touche ou un bouton de souris : la commande affiche son nom à mettre dans `coach.toml`.

```
python coach.py
```
Lance le coach. Ajoute `--verbose` pour voir le détail, ou consulte `coach.log`.

Après chaque réponse, la console affiche une ligne comme celle-ci, mesurée depuis le relâchement de la touche :

```
[timing] transcription 0.6 s | first audio 3.1 s | full answer 14.2 s
```

`--ask` utilise la même session persistante que le coach vocal.

### Les touches

| Action | Par défaut | Effet |
|---|---|---|
| `push_to_talk` | `key:f13` | Maintenir pour parler |
| `push_to_talk_with_screenshot` | `key:f14` | Pareil, avec une capture d'écran jointe à coup sûr |
| `screenshot` | `key:f15` | Un appui = capture immédiate, jointe à ta **prochaine** question (valable `armed_seconds`). Pratique pour figer une infobulle puis poser la question tranquillement |
| `stop` | `key:pause` | Coupe la voix et annule la réponse en cours |
| `new_session` | `key:scroll_lock` | Efface la conversation et en démarre une nouvelle (sinon elle est conservée, même après un redémarrage) |

**Conseil** : F13 et F14 n'existent pas sur la plupart des claviers. Assigne-les aux **boutons latéraux de ta souris** dans son logiciel (G Hub, Synapse, iCUE…). Le coach n'écoute alors que le clavier, ce qui coûte quasiment rien et n'interfère pas avec le jeu.

`mouse:x1` et `mouse:x2` fonctionnent aussi directement. En revanche, l'écoute de la souris en Python traite chaque mouvement, ce qui peut ajouter de la latence quand le PC est chargé.

Appuyer sur le push-to-talk pendant que le coach parle le coupe (`interrupt_on_push_to_talk = true`). Les petits bips signalent le début de l'écoute, la fin de l'écoute et les erreurs.

### Montrer ton écran au coach

Trois façons, au choix :

1. **Automatique, à la voix** (`auto_on_keywords = true`) : à chaque push-to-talk normal, une capture est prise en silence. Elle n'est envoyée que si ta question parle de ce que tu vois (« regarde mon écran », « c'est quoi cet objet », « tu vois ce boss ? »…). Sinon elle est supprimée tout de suite. La liste est dans `[screenshot] keywords`. La console affiche `[screenshot: auto]` quand ça se déclenche.
2. **Forcée** : maintiens `push_to_talk_with_screenshot` au lieu du push-to-talk normal.
3. **En deux temps** : appuie sur `screenshot` (bip + « Capture prise »), puis pose ta question avec le push-to-talk normal.

La capture vise l'écran où se trouve la fenêtre active (`monitor = "auto"`), réduite à 1600 px de large. Pour tester : `python coach.py --screenshot` (capture après 3 secondes, signale si l'image est noire).

### Réglages utiles

| Clé | Rôle |
|---|---|
| `general.game` et `general.preferred_sources` | Le jeu et les sites à privilégier (injectés dans `coach_prompt.md`) |
| `claude.model` | `sonnet` (bon compromis vitesse/qualité), `opus` (plus fort, plus lent) |
| `claude.effort` | `low` pour des réponses rapides. Si la CLI le refuse pour ton modèle, mets `""` |
| `claude.persist_session` | `true` : garde la conversation entre deux lancements du coach |
| `claude.session_idle_reset_minutes` | `0` = jamais d'expiration. Sinon, repart sur une session neuve après X minutes sans question |
| `claude.interrupt_grace_seconds` | Délai laissé à Claude pour arrêter un tour interrompu avant d'être relancé |
| `claude.isolate` | Ignore tes plugins, hooks et serveurs MCP de Claude Code pour démarrer plus vite |
| `codex.reasoning_effort` | `low`, `medium` ou `high` |
| `stt.initial_prompt` | Vocabulaire qui aide Whisper à reconnaître les noms propres du jeu |
| `tts.piper_length_scale` | Moins de 1 pour parler plus vite |

Pour un autre jeu (par exemple Path of Exile 2), change `game`, `preferred_sources` et `initial_prompt`.

### Dépannage

- **Le script plante au démarrage en citant cudnn ou cublas** : les bibliothèques NVIDIA manquent (voir l'étape 4), ou mets `device = "cpu"`.
- **Les captures sont noires** : le jeu est en plein écran exclusif. Passe en « plein écran fenêtré ».
- **Les touches ne réagissent pas en jeu** : si le jeu ou Battle.net tourne en administrateur, lance aussi le coach en administrateur.
- **Codex échoue sous Windows à cause du sandbox** : mets `sandbox = ""` dans `[codex]`.
- **Le coach annonce une erreur** : la cause exacte s'affiche dans la console (limite d'usage atteinte, non connecté…).

### Consommation

Chaque question est une vraie requête Claude Code ou Codex avec recherche web. Elle compte dans les limites de ton abonnement, au même titre que leur usage normal.

Avec une session persistante, l'historique grossit au fil des questions et chaque nouvelle question le renvoie au modèle. Une longue soirée sur la même session consomme donc plus qu'une suite de sessions courtes. Quand tu changes de sujet (autre zone, autre perso), appuie sur `new_session`.
