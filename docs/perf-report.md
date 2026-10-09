# Rapport de performances — prototype

Mesures du 9 octobre 2026 sur la machine cible : Intel Core i7-10700K, NVIDIA
RTX 3070, Windows 10 22H2, Logitech StreamCam en USB 3. Build release
(`lto = "fat"`, `target-cpu=native`), classifieurs géométriques intégrés,
fournisseur d'exécution CPU sauf mention contraire.

Toutes les commandes sont reproductibles :

```powershell
mudra bench --duration 15s      # B-03, bout en bout
mudra bench --providers         # B-01, fournisseurs d'exécution
mudra bench --decode            # B-02, décodage MJPEG seul
```

## Synthèse

| Exigence | Cible | Mesuré | Verdict |
|---|---|---|---|
| ENF-01 débit | 60 fps, < 1 % de trames abandonnées | 59,4 fps traités, 0,00 % | tenu |
| ENF-02 latence par trame | p95 ≤ 8 ms, p99 ≤ 12 ms | p50 7,1 · p95 18,2 · p99 24,6 ms | **non tenu** |
| ENF-04 CPU avec main | ≤ 8 % de la machine | 3,6 % | tenu |
| ENF-04 CPU sans main | ≤ 2 % | 1,8 % (sur 20 s) | tenu, de peu |
| ENF-04 RAM | ≤ 250 Mo | 64 Mo en arrière-plan, 196 Mo overlay affiché | tenu |
| ENF-04 VRAM | ≤ 400 Mo | aucune (inférence sur CPU) | tenu |
| ENF-05 démarrage | ≤ 2 s jusqu'à la première trame | 0,62 s | tenu |
| ENF-06 zéro allocation | 0 par trame | non mesuré (pas d'allocateur compteur) | non vérifié |

Non mesurés dans ce prototype : ENF-10 à ENF-12 (qualité de reconnaissance sur
tes données, faux positifs par heure) et ENF-14 (8 heures de fonctionnement).
Ils demandent des enregistrements et du temps devant la caméra.

## B-03 — bout en bout

StreamCam MJPEG 1280×720 à 60 fps, une main suivie pendant toute la mesure,
15 s, 885 trames.

| Étage | p50 | p95 | p99 | Budget p95 |
|---|---|---|---|---|
| Décodage MJPEG | 4,01 ms | 7,17 ms | 17,41 ms | 3,0 ms |
| Points-clés (recadrage compris) | 2,99 ms | 4,22 ms | 6,78 ms | 3,0 ms |
| Classifieurs + moteur | < 0,01 ms | 0,01 ms | 0,01 ms | 0,7 ms |
| **Total, de la trame à la décision** | **7,14 ms** | **18,17 ms** | **24,58 ms** | **8,0 ms** |

Capture : 59,7 fps. Traitement : 59,4 fps. Aucune trame abandonnée.

Sans main dans le champ (mode veille, 20 s) : 8,9 trames traitées par seconde,
1,8 % de CPU, 71 Mo.

### Pourquoi le p95 dépasse la cible

Deux causes, mesurées séparément.

**1. Le décodage MJPEG 720p dépasse son budget même dans le meilleur cas.**
`mudra bench --decode` décode 400 fois de vraies trames de la StreamCam, gardées
en mémoire, en boucle serrée :

| Résolution | Taille moyenne | p50 | p95 | p99 | Débit |
|---|---|---|---|---|---|
| 1280×720 | 156 Ko | 3,74 ms | 3,82 ms | 3,93 ms | 246 Mpixels/s |
| 640×360 | 40 Ko | 0,96 ms | 1,02 ms | 1,05 ms | 240 Mpixels/s |

zune-jpeg plafonne ici à environ 245 Mpixels/s : 3,7 ms pour une trame 720p,
contre 3,0 ms de budget.

**2. La gestion d'énergie du processeur allonge les trames isolées.** Dans le
pipeline, le thread vision dort entre deux trames. En boucle serrée, le décodage
prend 3,7 ms avec un p99 à 3,9 ms ; dans le pipeline à 60 Hz, le même décodage
a un p50 de 4,0 ms mais un p95 de 7,2 ms et un p99 de 17 ms. En veille, à 10 Hz,
il monte à 8 à 23 ms par trame. Ce rapport de un à six correspond à un cœur qui
repart de sa fréquence minimale. Le phénomène est le même pour l'inférence
(points-clés : 2,5 ms en boucle serrée, 3,0 ms dans le pipeline).

Essais sans effet sur cette queue de distribution, main visible : priorité MMCSS
« Capture », priorité au-dessus de la normale, priorité normale (p95 du décodage
entre 7,2 et 7,3 ms dans les trois cas). L'exclusion explicite du bridage
d'énergie de Windows (`ThreadPowerThrottling`) n'a pas montré d'amélioration non
plus, sur des mesures plus bruitées. Le code d'essai n'a pas été conservé.

### Compromis proposés

Le cahier des charges demande, quand une cible est inatteignable, de documenter
la mesure et de proposer un compromis (§6.1). Par ordre de coût :

1. **Mode économique 640×360 à 60 fps** (D-03) : le décodage tombe à 1,0 ms,
   soit un total p50 attendu d'environ 4 ms (non mesuré main visible). C'est un réglage de configuration
   (`camera.width = 640`, `camera.height = 360`). Contrepartie : moins de pixels
   sur une main à 1,5 m. À essayer en premier.
2. **libjpeg-turbo** à la place de zune-jpeg : c'est l'alternative que D-04
   demande d'évaluer (B-02). Elle n'a pas été mesurée ici, parce qu'elle ajoute
   une dépendance C à compiler. Attendu : un décodage 720p sous 3 ms.
3. **Plan d'alimentation « Performances élevées »** de Windows : il supprime la
   remontée en fréquence, donc la queue de distribution, au prix de la
   consommation au repos. Non essayé : c'est un réglage système, qui t'appartient.
4. **Accepter l'écart.** La latence perçue d'une pose est dominée par le maintien
   (300 ms) ; 10 ms de plus sur 5 % des trames ne se voient pas. L'écart compte
   surtout pour la souris.

## B-01 — fournisseurs d'exécution

1000 inférences après 100 de préchauffage, en boucle serrée, image 1280×720,
recadrage affine et lecture des sorties compris.

| EP | Modèle | p50 | p95 | p99 | Chargement |
|---|---|---|---|---|---|
| CPU | paume 192×192 | 5,11 ms | 5,36 ms | 5,60 ms | 63 ms |
| CPU | points-clés 224×224 | 2,52 ms | 2,80 ms | 2,95 ms | 63 ms |
| DirectML | paume 192×192 | 1,22 ms | 1,36 ms | 1,44 ms | 701 ms |
| DirectML | points-clés 224×224 | 1,35 ms | 1,56 ms | 1,65 ms | 701 ms |

Dans le pipeline réel, DirectML est plus lent que le CPU (points-clés : p95 de
8,1 ms contre 4,2 ms) et occupe 286 Mo. Le défaut est donc le CPU :
[ADR 0001](adr/0001-fournisseur-execution.md). CUDA et TensorRT ne sont pas
compilés dans le prototype.

## Mémoire de l'application

Mémoire de travail du processus, application lancée normalement (image fixe en
entrée, fournisseur CPU) :

| Situation | Mémoire | Threads |
|---|---|---|
| Arrière-plan, Verrouillé, overlay jamais affiché | 64 Mo | 13 |
| Overlay affiché | 196 Mo | 38 |
| Overlay détruit (20 s après avoir été masqué) | 143 Mo | 21 |
| Fenêtre d'aperçu ouverte | ≈ 280 Mo | — |

L'overlay et l'aperçu ont chacun leur contexte graphique wgpu, d'environ 130 Mo.
Leurs fenêtres sont créées à la demande et détruites quand elles ne servent
plus ; le pilote graphique garde toutefois environ 80 Mo après la première
utilisation. Pour rester à 64 Mo, mettre `general.overlay = false`.

## Chaîne d'apprentissage

Sur le dataset synthétique de `mudra-train synth` (pas sur de vraies mains) :

| Modèle | Paramètres | Entraînement (CPU) | Écart ONNX Runtime / PyTorch | Rust / Python |
|---|---|---|---|---|
| Statique (MLP) | 21 066 | 1 s, 14 époques | 1,2 × 10⁻⁷ | identiques à 10⁻⁴ près |
| Dynamique (TCN) | 72 261 | 15 s, 27 époques | 1,2 × 10⁻⁷ | identiques à 10⁻⁴ près |

Les scores obtenus sur ces mains de synthèse (macro-F1 de 100 %) ne disent rien
de la reconnaissance réelle : ils prouvent seulement que la chaîne fonctionne
de bout en bout. ENF-10 et ENF-11 restent à mesurer sur tes enregistrements.
