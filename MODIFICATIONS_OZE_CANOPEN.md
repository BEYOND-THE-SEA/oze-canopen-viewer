# Modifications à faire dans votre fork oze-canopen

## Résumé rapide

1. **Cargo.toml** : Remplacer `socketcan` par `host-can` avec features conditionnelles
2. **Code** : Remplacer `CanSocket` par `Adapter` et `CanFrame` par `Frame`

## 1. Modifier Cargo.toml de oze-canopen

**Trouver** :

```toml
[dependencies]
socketcan = "3.5"
```

**Remplacer par** :

```toml
[target.'cfg(target_os = "linux")'.dependencies]
host-can = { version = "0.1.3", features = ["socketcan"] }

[target.'cfg(target_os = "windows")'.dependencies]
host-can = { version = "0.1.3", features = ["pcan"] }

[target.'cfg(target_os = "macos")'.dependencies]
host-can = { version = "0.1.3", features = ["pcan"] }
```

## 2. Trouver les fichiers à modifier

Dans votre fork oze-canopen, chercher :

```bash
grep -r "socketcan" src/
grep -r "CanSocket" src/
```

Fichiers typiques : `src/interface.rs`, `src/canopen.rs`, ou similaire.

## 3. Remplacer les imports

**Avant** :

```rust
use socketcan::{CanSocket, CanFrame};
```

**Après** :

```rust
use host_can::{Adapter, Frame};
```

## 4. Remplacer l'utilisation

**Avant** :

```rust
let socket = CanSocket::bind("can0")?;
let frame = socket.read_frame()?;
socket.write_frame(&frame)?;
```

**Après** :

```rust
// Sur Windows, mapper "can0" -> "PCAN_USBBUS1" si nécessaire
let interface = if cfg!(target_os = "windows") {
    match name {
        "can0" => "PCAN_USBBUS1",
        "can1" => "PCAN_USBBUS2",
        _ => name, // Si déjà au format PCAN_USBBUS*
    }
} else {
    name
};

let adapter = Adapter::new(interface)?;
let frame = adapter.recv()?;
adapter.send(&frame)?;
```

## 5. Conversion des types (si nécessaire)

Si le code utilise `CanFrame`, convertir :

```rust
// host_can::Frame -> format interne
let id = frame.id();
let data = frame.data().to_vec();

// format interne -> host_can::Frame
let frame = Frame::new(id, &data)?;
```

## 6. Gestion du bitrate

- **Linux** : SocketCAN gère via `ip link` (comme avant)
- **Windows** : PCAN-Basic gère automatiquement

Si le code configure le bitrate manuellement, adapter selon la plateforme.

## 7. Après modification

1. Commit et push sur votre fork GitHub
2. Décommenter le patch dans `Cargo.toml` de ce projet
3. Tester :
   ```bash
   cargo build --release  # Linux
   cargo build --release --target x86_64-pc-windows-gnu  # Windows
   ```

## Notes importantes

- `host-can` avec feature `socketcan` sur Linux = même comportement que `socketcan`
- Sur Windows, utiliser `PCAN_USBBUS1` au lieu de `can0`
- Les features conditionnelles gèrent automatiquement la sélection
