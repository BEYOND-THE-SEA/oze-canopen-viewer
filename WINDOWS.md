# Compilation pour Windows

**Important** : Une fois configuré, vous pouvez compiler pour Linux ou Windows sans modifier quoi que ce soit. Les dépendances sont automatiquement sélectionnées selon la plateforme cible.

## Prérequis

### Sur Linux (pour compiler)

```bash
# Installer le toolchain Windows
rustup target add x86_64-pc-windows-gnu
sudo apt-get install mingw-w64
```

### Sur Windows (pour exécuter)

1. Installer [PCAN-Basic](https://www.peak-system.com/Downloads.76.0.html)
2. Brancher votre SH-C30A (firmware PCAN)

## Modification de oze-canopen

**Vous avez 2 options** :

### Option A : Fork GitHub (recommandé - pas de clone local)

1. Forker `oze-canopen` sur GitHub (si pas déjà fait)
2. Modifier le fork sur GitHub pour utiliser `host-can`
3. Utiliser un patch Git dans `Cargo.toml` (voir ci-dessous)

### Option B : Clone local

1. Cloner `oze-canopen` localement
2. Le modifier pour utiliser `host-can`
3. Utiliser un patch path dans `Cargo.toml`

### Modifications nécessaires dans oze-canopen

### 1. Modifier Cargo.toml de oze-canopen

Remplacer :

```toml
[dependencies]
socketcan = "3.5"
```

Par :

```toml
[target.'cfg(target_os = "linux")'.dependencies]
host-can = { version = "0.1.3", features = ["socketcan"] }

[target.'cfg(target_os = "windows")'.dependencies]
host-can = { version = "0.1.3", features = ["pcan"] }
```

### 2. Remplacer socketcan par host-can dans le code

Trouver où `socketcan::CanSocket` est utilisé et remplacer par `host_can::Adapter`.

Exemple :

```rust
// Avant
use socketcan::{CanSocket, CanFrame};
let socket = CanSocket::bind("can0")?;

// Après
use host_can::{Adapter, Frame};
let adapter = Adapter::new("can0")?;  // Sur Windows: "PCAN_USBBUS1"
```

## Compilation

### Configuration initiale (une seule fois)

**Option A - Fork GitHub** (recommandé) :

```toml
[patch.crates-io]
oze-canopen = { git = "https://github.com/<votre-username>/oze-canopen", branch = "host-can" }
```

**Option B - Clone local** :

```toml
[patch.crates-io]
oze-canopen = { path = "../oze-canopen" }
```

**C'est tout !** Cette configuration reste active pour toutes les compilations suivantes.

### Compiler

Les dépendances sont automatiquement sélectionnées selon la plateforme cible :

```bash
# Compiler pour Linux (défaut)
cargo build --release

# Compiler pour Windows (depuis Linux)
cargo build --release --target x86_64-pc-windows-gnu

# Pas besoin de modifier quoi que ce soit entre les deux !
```

Les features conditionnelles dans `Cargo.toml` gèrent automatiquement :

- Linux → `host-can` avec feature `socketcan`
- Windows → `host-can` avec feature `pcan`

## Utilisation sur Windows

```cmd
oze-canopen-viewer.exe --can PCAN_USBBUS1 --bitrate 500000
```

**Note** : Sur Windows, utiliser `PCAN_USBBUS1` au lieu de `can0`.
