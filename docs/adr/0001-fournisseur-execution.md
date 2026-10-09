# ADR 0001 — Fournisseur d'exécution de la vision : CPU par défaut

- Date : 9 octobre 2026
- Statut : accepté
- Révise : D-07 (« Défaut : DirectML, repli automatique sur CPU »)

## Contexte

D-07 fixe DirectML par défaut et précise que ce défaut « est confirmé ou révisé
uniquement par le benchmark B-01 ». La règle de B-01 : retenir le plus faible
p95 respectant ENF-04 ; à ±0,5 ms près, le moins gourmand en CPU et en RAM.

## Mesures

Machine cible (i7-10700K, RTX 3070, Windows 10), build release.

`mudra bench --providers` — 1000 inférences après 100 de préchauffage, en boucle
serrée, recadrage affine compris :

| EP | Modèle | p50 | p95 | p99 | Chargement |
|---|---|---|---|---|---|
| CPU | paume 192×192 | 5,11 ms | 5,36 ms | 5,60 ms | 63 ms |
| CPU | points-clés 224×224 | 2,52 ms | 2,80 ms | 2,95 ms | 63 ms |
| DirectML | paume 192×192 | 1,22 ms | 1,36 ms | 1,44 ms | 701 ms |
| DirectML | points-clés 224×224 | 1,35 ms | 1,56 ms | 1,65 ms | 701 ms |

`mudra bench --duration 15s` — pipeline réel, StreamCam MJPEG 1280×720 à 60 fps,
une main suivie :

| | DirectML | CPU |
|---|---|---|
| Points-clés, p50 / p95 | 5,89 / 8,13 ms | 2,99 / 4,22 ms |
| Détection de paume, p50 | 7,81 ms | 5,92 ms |
| Total trame → décision, p50 / p95 | 11,0 / 19,6 ms | 7,1 / 18,2 ms |
| Première trame traitée | 1,18 s | 0,62 s |
| CPU du processus | 1,6 % | 3,6 % |
| Mémoire de travail | 286 Mo | 71 Mo |

## Analyse

En boucle serrée, DirectML est deux à quatre fois plus rapide. Dans le pipeline
réel, il est plus lent que le CPU : à 60 inférences par seconde séparées par des
temps morts, le GPU retombe au repos entre deux trames et chaque inférence paie
son réveil. Le spécifié l'avait anticipé (« sur des modèles aussi petits, le CPU
peut rivaliser »).

DirectML dépasse en outre la cible mémoire d'ENF-04 (286 Mo pour 250 Mo), et
double le temps de démarrage. Le CPU respecte ENF-04 sur les trois postes
(3,6 % ≤ 8 %, 71 Mo ≤ 250 Mo, pas de VRAM).

## Décision

`inference.execution_provider = "cpu"` par défaut, 2 threads intra-op sans
attente active. DirectML reste disponible (`"directml"`), avec repli automatique
sur CPU s'il ne peut pas être initialisé. Les sessions des classifieurs restent
sur CPU, 1 thread (D-08).

## Conséquences

- Le budget « points-clés ≤ 3,0 ms p95 » n'est pas tenu dans le pipeline réel
  (4,2 ms) ; voir docs/perf-report.md.
- La détection de paume coûte 6 ms sur CPU, mais elle ne tourne que sans main
  suivie, à 10 Hz.
- À refaire si la charge change : deux mains en permanence, ou des modèles plus
  gros, peuvent redonner l'avantage au GPU. `mudra bench --providers` et
  `mudra bench` suffisent à trancher.
