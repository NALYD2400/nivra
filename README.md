# Nivra

Application Windows pour suivre ses comptes et les quotas disponibles. Elle utilise React, Vite et Tauri 2.

## Installation

Télécharger l’installateur Windows depuis les releases GitHub et l’exécuter. Nivra vérifie ensuite les nouvelles versions au lancement et régulièrement pendant son utilisation. La mise à jour peut aussi être recherchée dans **Réglages → Mises à jour** ; son installation est déclenchée par l’utilisateur.

Les mises à jour Tauri sont signées et vérifiées avant installation. L’installateur n’est pas signé avec un certificat de signature Windows ; Windows peut afficher un avertissement SmartScreen.

## Développement

Installer Node.js, Rust et les prérequis Windows de Tauri, puis lancer `npm ci` et `npm run tauri -- dev`. Pour un installateur, configurer `TAURI_SIGNING_PRIVATE_KEY` et `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, puis lancer `npm run tauri -- build`.

## Comptes et limites

- **Codex** : plusieurs comptes peuvent être connectés avec des profils séparés. Les quotas fournis par Codex sont actualisés périodiquement.
- **Claude Code** : une connexion via un profil séparé est proposée ; l’affichage des quotas n’est pas encore disponible.
- **Cursor** : Nivra peut ouvrir un profil Cursor distinct. Les quotas ne sont pas encore disponibles.
- **Antigravity** : Nivra peut détecter une adresse enregistrée sur cet appareil et tente de lire les quotas du service local quand il est ouvert. La connexion de plusieurs comptes n’est pas encore disponible.

Les données affichées dépendent des services et de leurs interfaces disponibles. La connexion aux comptes se fait par les outils des fournisseurs ; Nivra ne demande pas les mots de passe.

## Publier une version

La version de `src-tauri/tauri.conf.json` doit correspondre aux versions de `package.json` et `src-tauri/Cargo.toml`. Après avoir mis à jour ces fichiers et `package-lock.json` / `Cargo.lock`, pousser le code, puis un tag `v` suivi de cette version (par exemple `v0.1.1`). Le workflow GitHub construit l’installateur, sa signature et le fichier `latest.json` pour la mise à jour.

Le dépôt doit disposer des secrets GitHub Actions `TAURI_SIGNING_PRIVATE_KEY` et `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Conserver une sauvegarde privée de la clé de signature : la perdre empêchera les installations existantes d’accepter les futures mises à jour. Ne jamais la publier dans le dépôt.
