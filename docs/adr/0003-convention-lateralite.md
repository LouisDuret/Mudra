# ADR 0003 — Convention de latéralité du modèle de points-clés

- Date : 9 octobre 2026
- Statut : accepté
- Concerne : EF-14, §14 (« latéralité inversée par le miroir »)

## Contexte

Le cahier des charges indique que le modèle de points-clés « suppose une image
de type selfie » et demande un test doré avec une main droite connue (EF-14).
La documentation de MediaPipe dit la même chose : la latéralité annoncée
suppose une image retournée horizontalement.

## Mesure

`handpose_estimation_mediapipe_2023feb.onnx` sur cinq photos **non retournées**
de `tests/fixtures/hands`, dont la main est identifiable sans ambiguïté par le
bras et le côté visible de la paume :

| Photo | Main réelle | Score brut du modèle |
|---|---|---|
| victory.jpg | droite | > 0,5 |
| thumbs_up.jpg | droite | > 0,5 |
| thumbs_down.jpg | droite | > 0,5 |
| pointing_up.jpg | gauche | < 0,5 |
| woman_hands.jpg (main du haut) | droite | > 0,5 |
| woman_hands.jpg (main du bas) | gauche | < 0,5 |

Sur ces six mains, un score supérieur à 0,5 désigne la main droite **telle
qu'elle apparaît dans une image non retournée** : l'inverse de la convention
annoncée.

## Décision

`droite = (score > 0,5) ≠ miroir`. En vue selfie (`camera.mirror = true`),
l'image traitée est retournée, et la latéralité du modèle aussi. Mudra annonce
donc toujours la main réelle de l'utilisateur, quel que soit le réglage du
miroir : le miroir n'est qu'un mode d'affichage.

## Conséquences

Le test doré `mirror_flips_coordinates_but_not_handedness` verrouille ce
comportement, et `victory_right_hand_is_found_and_tracked` sert de main droite
connue pour EF-14. Si un autre modèle de points-clés est substitué, ces tests
diront aussitôt si sa convention diffère.
