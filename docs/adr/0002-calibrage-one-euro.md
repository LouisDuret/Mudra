# ADR 0002 — Calibrage du filtre One Euro

- Date : 9 octobre 2026
- Statut : accepté, à affiner devant la caméra
- Précise : D-11 et §7.5 (« valeurs de départ […] à calibrer en phase 4 »)

## Contexte

Le cahier des charges donne comme valeurs de départ `min_cutoff = 1,0 Hz`,
`beta = 0,05`, `d_cutoff = 1,0`, les coordonnées étant exprimées en unités de
taille de paume. Pour la souris : `min_cutoff = 0,8`, `beta = 0,3`.

La fréquence de coupure du filtre vaut `min_cutoff + beta × |vitesse|`. Elle
dépend donc de l'unité de la vitesse. En tailles de paume par seconde, une main
qui bouge va de 1 à 20 unités/s : très loin des centaines de pixels par seconde
pour lesquelles les valeurs usuelles de `beta` (quelques millièmes) sont
données.

## Mesure

Constante de temps `τ = 1 / (2π·fc)` du filtre, pour un doigt qui se déplace à
5 paumes/s (un pincement, un changement de pose) :

| `beta` | Coupure à 5 paumes/s | τ | Temps de réponse à 95 % |
|---|---|---|---|
| 0,05 (valeur de départ) | 1,25 Hz | 127 ms | ≈ 380 ms |
| 1,5 (retenu) | 8,5 Hz | 19 ms | ≈ 56 ms |

Au repos, le filtre est le même dans les deux cas (1 Hz, τ = 159 ms) : le
tremblement des points-clés est atténué pareillement.

Avec `beta = 0,05`, le test de bout en bout `pinch_controls_volume_then_releases`
échoue : après 400 ms de pincement, les points lissés n'ont pas encore rejoint
la pose, et le contrôle continu ne démarre pas. ENF-03 demande une pose
reconnue en `hold_ms + 30 ms` : impossible avec 380 ms de traînée.

## Décision

- Points-clés : `min_cutoff = 1,0`, `beta = 1,5`, `d_cutoff = 1,0`.
- Souris : `min_cutoff = 0,8`, `beta = 1,0`, appliqué au bout de l'index **non
  lissé**. Filtrer une seconde fois des points déjà lissés ajouterait les deux
  retards ; le profil souris est donc le seul filtre sur le trajet du curseur.

Ces valeurs sont celles de `config/default.toml` ; elles restent modifiables à
chaud.

## Conséquences

Ce calibrage est fait par le calcul et sur des mains de synthèse, pas à l'œil
devant la caméra comme le prévoit la phase 4. Si les points tremblent en
mouvement lent, baisser `beta` ; s'ils traînent, le monter. L'aperçu montre le
résultat en direct.
