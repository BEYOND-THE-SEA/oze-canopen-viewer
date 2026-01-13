# Compilation pour Windows - Analyse Complète

## Situation actuelle

Le projet utilise `oze-canopen` (v0.1.0) qui dépend directement de `socketcan` (v3.5.0). La chaîne de dépendances est :

```
oze-canopen-viewer → oze-canopen → socketcan
```

**Problème** : `socketcan` est une API **exclusivement Linux**. Le crate refuse explicitement de compiler pour d'autres plateformes :

```
error: "Building for anything but Linux is not supported by socketcan"
```

## Analyse de `oze-canopen`

- **Source** : Publié sur crates.io, mais le repository source n'est pas documenté publiquement
- **Dépendances** : `socketcan`, `tokio`, `binrw`, `serde`, `futures-util`
- **Architecture** : Fortement couplé à `socketcan`, pas d'abstraction multi-plateforme

---

# Solutions Possibles

## 🟢 Solution 1 : WSL2 avec SocketCAN (Recommandée - Facile)

**Effort** : Faible | **Risque** : Faible | **Temps** : 1-2 heures

Les utilisateurs Windows peuvent utiliser WSL2 pour exécuter l'application Linux.

### Configuration WSL2 avec support CAN

Par défaut, le kernel WSL2 ne supporte pas CAN. Il faut compiler un kernel custom :

1. **Prérequis** :

   ```bash
   wsl --set-default-version 2
   sudo apt update && sudo apt install can-utils build-essential flex bison libssl-dev libelf-dev
   ```

2. **Compiler un kernel WSL2 avec CAN** :

   ```bash
   git clone https://github.com/microsoft/WSL2-Linux-Kernel.git
   cd WSL2-Linux-Kernel
   # Activer CONFIG_CAN, CONFIG_CAN_RAW, CONFIG_CAN_VCAN dans .config
   make -j$(nproc)
   ```

3. **Configurer WSL** (fichier `%USERPROFILE%\.wslconfig`) :

   ```ini
   [wsl2]
   kernel=C:\\Users\\<username>\\WSL2-Linux-Kernel\\vmlinux
   ```

4. **Charger les modules CAN** :

   ```bash
   sudo modprobe can
   sudo modprobe can-raw
   sudo modprobe vcan
   ```

**Limitations** :

- Pas d'accès direct aux adaptateurs USB-CAN physiques
- Fonctionne bien avec `vcan` pour le développement/test
- Peut utiliser `can-utils` sur une autre machine Linux et forwarder via réseau

---

## 🟢 Solution 1.5 : SH-C30A (DSD TECH) - Adaptateur existant

**Effort** : Faible à Moyen | **Risque** : Faible | **Temps** : 1-3 jours

### Description

Vous possédez déjà un **SH-C30A de DSD TECH**, basé sur **Canable/Candlelight**. Cet adaptateur peut fonctionner sous Windows avec différents firmwares.

### Caractéristiques du SH-C30A

- **Microcontrôleur** : STM32F072C8T6
- **Firmware par défaut** : Candlelight (SocketCAN - Linux uniquement)
- **Firmwares alternatifs** : PCAN, SLCAN, BUSMASTER
- **Protocoles** : CAN 2.0A/B jusqu'à 1 Mbps
- **Terminaison** : 120Ω intégrée (commutable)

### Option A : Utiliser avec firmware PCAN (RECOMMANDÉ - DÉJÀ INSTALLÉ ✅)

**Status** : ✅ **Vous avez déjà le firmware PCAN installé !**

**Avantages** :

- ✅ Compatible avec `host-can` ou `peak-can-sys`
- ✅ Support natif Windows via PCAN-Basic
- ✅ Pas besoin de flasher ou d'acheter un nouvel adaptateur
- ✅ Prêt à l'emploi !

**Procédure** :

1. **Installer PCAN-Basic sur Windows** :

   - Télécharger : https://www.peak-system.com/Downloads.76.0.html
   - Installer le driver
   - Vérifier que l'adaptateur apparaît comme `PCAN_USBBUS1` dans le gestionnaire de périphériques

2. **Utiliser avec le projet** :
   - Suivre la **Solution 3 (`host-can`)** ou **Solution 4 (`peak-can-sys`)**
   - L'adaptateur sera accessible via `PCAN_USBBUS1` sous Windows

**Ressources** :

- [PCAN-Basic Download](https://www.peak-system.com/Downloads.76.0.html)
- [PCAN-Basic Documentation](https://www.peak-system.com/produktcd/Develop/PC%20interfaces/Windows/PCAN-Basic%20API/)

### Option B : Utiliser SLCAN (Serial Line CAN)

**Avantages** :

- ✅ Pas besoin de flasher (si firmware SLCAN disponible)
- ✅ Protocole série standard
- ✅ Supporté par plusieurs outils Windows

**Inconvénients** :

- ⚠️ Pas de crate Rust mature pour SLCAN
- ⚠️ Nécessite un wrapper série

**Procédure** :

1. **Flasher avec firmware SLCAN** (si pas déjà fait)
2. **Créer un wrapper Rust** pour SLCAN :

   ```rust
   // Exemple de wrapper SLCAN
   use serialport::SerialPort;

   pub struct SlcanAdapter {
       port: Box<dyn SerialPort>,
   }

   impl SlcanAdapter {
       pub fn new(port_name: &str, bitrate: u32) -> Result<Self, Error> {
           let mut port = serialport::new(port_name, 115200)
               .open()?;

           // Envoyer commande SLCAN pour configurer
           port.write_all(format!("S{}\r", bitrate).as_bytes())?;

           Ok(Self { port })
       }

       pub fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), Error> {
           // Format SLCAN : t<id><data>\r
           let mut cmd = format!("t{:03X}{}", id, hex::encode(data));
           cmd.push('\r');
           self.port.write_all(cmd.as_bytes())?;
           Ok(())
       }

       pub fn recv_frame(&mut self) -> Result<CanFrame, Error> {
           // Lire et parser format SLCAN
           // Format : t<id><data>\r ou r<id><data>\r
           // ...
       }
   }
   ```

3. **Intégrer dans oze-canopen** :
   - Créer une abstraction commune avec socketcan
   - Utiliser des features conditionnelles

**Ressources** :

- [SLCAN Protocol Specification](https://www.can232.com/docs/canusb_manual.pdf)
- [serialport crate](https://docs.rs/serialport)

### Option C : Utiliser Cangaroo (outil Windows)

**Description** : Cangaroo est un outil open-source Windows mentionné dans la documentation du SH-C30A.

**Limitation** : C'est un outil GUI, pas une bibliothèque Rust. Il faudrait :

- Soit créer un bridge entre Cangaroo et votre application
- Soit utiliser Cangaroo comme référence pour créer votre propre wrapper

### Option D : WSL2 avec USB passthrough

**Avantages** :

- ✅ Garde le firmware Candlelight (SocketCAN)
- ✅ Pas besoin de flasher
- ✅ Fonctionne avec le code existant

**Procédure** :

1. **Configurer USB passthrough dans WSL2** :

   ```powershell
   # Sur Windows PowerShell
   usbipd wsl attach --busid <BUSID>
   ```

2. **Dans WSL2** :

   ```bash
   # L'adaptateur apparaîtra comme /dev/ttyACM0 ou similaire
   sudo modprobe can
   sudo modprobe can-raw
   sudo modprobe slcan
   sudo slcan_attach -f -s6 -o /dev/ttyACM0
   sudo slcand ttyACM0 can0
   sudo ip link set can0 up type can bitrate 500000
   ```

3. **Utiliser normalement** :
   ```bash
   ./oze-canopen-viewer --can can0 --bitrate 500000
   ```

**Ressources** :

- [USB passthrough WSL2](https://learn.microsoft.com/en-us/windows/wsl/connect-usb)

### Comparaison des options pour SH-C30A

| Option       | Effort    | Firmware requis | Compatibilité Rust | Recommandation |
| ------------ | --------- | --------------- | ------------------ | -------------- |
| **PCAN**     | 🟡 Moyen  | PCAN            | ✅ host-can        | ⭐⭐⭐⭐⭐     |
| **SLCAN**    | 🔴 Élevé  | SLCAN           | ⚠️ Wrapper custom  | ⭐⭐           |
| **WSL2 USB** | 🟢 Faible | Candlelight     | ✅ socketcan       | ⭐⭐⭐⭐       |
| **Cangaroo** | 🔴 Élevé  | Variable        | ❌ Outil externe   | ⭐             |

### Recommandation pour SH-C30A avec firmware PCAN

**🏆 Option A (PCAN - DÉJÀ INSTALLÉ)** : **PARFAIT pour un .exe Windows natif**

- ✅ Firmware PCAN déjà installé
- ✅ Utiliser `host-can` ou `peak-can-sys`
- ✅ Compatibilité complète Windows
- ✅ Pas besoin de flasher ou d'acheter quoi que ce soit

**Prochaines étapes** :

1. Installer PCAN-Basic sur Windows
2. Modifier `oze-canopen` pour utiliser `host-can`
3. Compiler pour Windows

**Alternative** : **Option D (WSL2 USB)** si vous voulez tester rapidement

- Pas besoin de modifier le code
- Fonctionne avec le code existant
- Mais nécessite WSL2 sur chaque machine Windows

---

# Analyse Détaillée des Alternatives Windows Natives

---

## 🟢 Solution 2 : Crate `automotive` + comma.ai Panda (RECOMMANDÉE)

**Effort** : Moyen | **Risque** : Faible | **Temps** : 1-2 semaines

### Description

Le crate [`automotive`](https://docs.rs/automotive) est **la solution la plus aboutie** pour du CAN cross-platform en Rust. Il offre une API **asynchrone complète** avec support multi-adaptateurs.

### Support des adaptateurs

| Adaptateur     | Linux | Windows | macOS | Méthode          |
| -------------- | ----- | ------- | ----- | ---------------- |
| SocketCAN      | ✅    | ❌      | ❌    | socketcan-rs     |
| comma.ai Panda | ✅    | ✅      | ✅    | rusb (USB natif) |

### Le comma.ai Panda

- **Prix** : **$99** (+ $30 shipping international)
- **Achat** : https://comma.ai/shop/products/panda-obd-ii-dongle
- **Compatibilité** : Véhicules 2008+, OBD-II standard
- **Variante rouge** : Support CAN FD ($99)

### Exemple de code

```rust
use automotive::StreamExt;

#[tokio::main]
async fn main() -> automotive::Result<()> {
    // Détection automatique de l'adaptateur (Panda ou SocketCAN)
    let adapter = automotive::can::get_adapter()?;
    let mut stream = adapter.recv();

    while let Some(frame) = stream.next().await {
        let id: u32 = frame.id.into();
        println!("[{}]\t0x{:x}\t{}", frame.bus, id, hex::encode(frame.data));
    }
    Ok(())
}
```

### Intégration avec oze-canopen

```toml
[dependencies]
automotive = "0.2"  # Vérifie la dernière version
```

### Modifications requises dans oze-canopen

1. Remplacer `socketcan` par `automotive`
2. Adapter les appels de réception/envoi pour l'API async
3. Le trait `CanAdapter` de `automotive` fournit déjà l'abstraction

### Avantages

- ✅ **Cross-platform natif** (Windows/Linux/macOS)
- ✅ **API async** (compatible avec tokio déjà utilisé)
- ✅ **Adaptateur abordable** ($99 vs €195+ pour PCAN)
- ✅ **Bien documenté** et activement maintenu
- ✅ Support UDS et ISO-TP intégrés
- ✅ Pas besoin de driver Windows spécial (USB natif via rusb)

### Inconvénients

- ⚠️ Limité au Panda sur Windows (pas de SocketCAN)
- ⚠️ Panda ne retry pas les frames non-ACKed
- ⚠️ Nécessite l'achat d'un adaptateur spécifique

### Verdict

**🏆 MEILLEURE OPTION** pour un .exe Windows natif avec effort minimal.

---

## 🟡 Solution 3 : Crate `host-can`

**Effort** : Moyen | **Risque** : Moyen | **Temps** : 1-2 semaines

### Description

Le crate [`host-can`](https://docs.rs/host-can) (v0.1.3) offre une abstraction cross-platform avec différents backends.

### Support des plateformes

| Plateforme | Backend   | Status          | Notes                             |
| ---------- | --------- | --------------- | --------------------------------- |
| Linux      | SocketCAN | ✅ Complet      | Tous adaptateurs kernel           |
| macOS      | PCANBasic | ✅ Complet      | Via mac-can.com                   |
| Windows    | PCANBasic | 🟡 Préliminaire | CPU élevé avec timeout sur recv() |

### Configuration Cargo.toml

```toml
[target.'cfg(target_os = "linux")'.dependencies]
host-can = { version = "0.1", features = ["socketcan"] }

[target.'cfg(target_os = "windows")'.dependencies]
host-can = { version = "0.1", features = ["pcan"] }

[target.'cfg(target_os = "macos")'.dependencies]
host-can = { version = "0.1", features = ["pcan"] }
```

### Prérequis Windows

1. **Driver PCAN-Basic** : https://www.peak-system.com/Downloads.76.0.html
2. **Adaptateur PCAN-USB** :
   - Standard : **€195** / $239
   - Isolé : **€238** / $296
   - FD : **$347**

### Exemple de code

```rust
use host_can::{Adapter, Frame};

fn main() -> Result<(), host_can::Error> {
    let adapter = Adapter::new("can0")?; // ou "PCAN_USBBUS1" sur Windows

    // Envoi
    let frame = Frame::new(0x123, &[0x01, 0x02, 0x03])?;
    adapter.send(&frame)?;

    // Réception
    let received = adapter.recv()?;
    println!("Received: {:?}", received);

    Ok(())
}
```

### Limitations connues

- ❌ **Pas d'API async** (prévu pour le futur)
- ❌ **Pas de CAN FD** (prévu pour le futur)
- ❌ **CPU élevé** sur Windows avec `recv()` + timeout
- ❌ APIs non stabilisées (changements possibles)

### Avantages

- ✅ Abstraction unifiée Linux/Windows/macOS
- ✅ Pas besoin de créer sa propre abstraction
- ✅ Support PCAN bien intégré

### Inconvénients

- ⚠️ En développement précoce
- ⚠️ Hardware PCAN coûteux (€195+)
- ⚠️ Pas d'async (problématique avec tokio)
- ⚠️ Bug CPU connu sur Windows

### Verdict

**Option viable** mais moins mature que `automotive`. Le manque d'async est un frein majeur pour ce projet qui utilise tokio.

---

## 🟡 Solution 4 : Crate `peak-can-sys` (bas niveau)

**Effort** : Élevé | **Risque** : Moyen | **Temps** : 2-3 semaines

### Description

Le crate [`peak-can-sys`](https://docs.rs/peak-can-sys) (v0.1.2) fournit des **bindings FFI bruts** vers l'API PCAN-Basic. C'est du bas niveau : il faut tout construire soi-même.

### Ce que ça fournit

```rust
// Bindings FFI directs - pas d'abstraction Rust
use peak_can_sys::*;

unsafe {
    let status = CAN_Initialize(PCAN_USBBUS1, PCAN_BAUD_500K, 0, 0, 0);
    // ...
}
```

### Ce qu'il faut construire

1. **Wrapper safe** autour des fonctions unsafe
2. **Gestion des erreurs** Rust-idiomatic
3. **Abstraction commune** avec socketcan
4. **Support async** (si nécessaire)

### Configuration

```toml
[target.'cfg(target_os = "windows")'.dependencies]
peak-can-sys = "0.1"

[target.'cfg(target_os = "linux")'.dependencies]
socketcan = "3.5"
```

### Prérequis Windows

1. Télécharger PCAN-Basic : https://www.peak-system.com/Downloads.76.0.html
2. Installer le driver
3. Copier `PCANBasic.dll` dans `C:\Windows\System32\` ou le répertoire de l'exe

### Architecture à implémenter

```rust
// Trait commun à définir
pub trait CanAdapter: Send + Sync {
    fn open(interface: &str, bitrate: u32) -> Result<Self, CanError> where Self: Sized;
    fn send(&mut self, frame: &CanFrame) -> Result<(), CanError>;
    fn recv(&mut self) -> Result<CanFrame, CanError>;
    fn recv_timeout(&mut self, timeout: Duration) -> Result<Option<CanFrame>, CanError>;
}

// Implémentation Linux
#[cfg(target_os = "linux")]
pub struct SocketCanAdapter { /* ... */ }

#[cfg(target_os = "linux")]
impl CanAdapter for SocketCanAdapter { /* ... */ }

// Implémentation Windows
#[cfg(target_os = "windows")]
pub struct PcanAdapter { /* ... */ }

#[cfg(target_os = "windows")]
impl CanAdapter for PcanAdapter {
    fn open(interface: &str, bitrate: u32) -> Result<Self, CanError> {
        unsafe {
            let baud = match bitrate {
                500_000 => peak_can_sys::PCAN_BAUD_500K,
                250_000 => peak_can_sys::PCAN_BAUD_250K,
                125_000 => peak_can_sys::PCAN_BAUD_125K,
                // ...
            };
            let status = peak_can_sys::CAN_Initialize(
                peak_can_sys::PCAN_USBBUS1,
                baud, 0, 0, 0
            );
            // Gestion erreurs...
        }
    }
    // ...
}
```

### Avantages

- ✅ Contrôle total sur l'implémentation
- ✅ Pas de dépendance à un crate tiers instable
- ✅ Peut être optimisé pour les besoins spécifiques

### Inconvénients

- ❌ **Beaucoup de code à écrire** (unsafe FFI)
- ❌ **Maintenance** de deux backends
- ❌ Risque de bugs dans le code unsafe
- ❌ Hardware PCAN coûteux (€195+)

### Verdict

**Option pour experts** qui veulent un contrôle total. Déconseillé si `automotive` ou `host-can` suffisent.

---

## 🟡 Solution 5 : Crate `zlgcan`

**Effort** : Moyen | **Risque** : Moyen | **Temps** : 2-3 semaines

### Description

Le crate [`zlgcan`](https://docs.rs/zlgcan) supporte les adaptateurs ZLG (周立功), un fabricant chinois populaire.

### Adaptateurs supportés

| Modèle        | CAN | CAN FD | Prix estimé |
| ------------- | --- | ------ | ----------- |
| USBCAN-I      | ✅  | ❌     | ~€35-50     |
| USBCAN-II     | ✅  | ❌     | ~€40-60     |
| USBCANFD-200U | ✅  | ✅     | ~€80-120    |
| USBCANFD-400U | ✅  | ✅     | ~€150+      |
| USBCANFD-800U | ✅  | ✅     | ~€200+      |

**Note** : Prix approximatifs sur Aliexpress/Fruugo. Qualité variable selon le vendeur.

### Versions du crate

- `0.1.x` : Déprécié
- `0.2.x` : Synchrone
- `0.3.x+` : **Asynchrone** (recommandé)

### Configuration

```toml
[dependencies]
rs-can = "0.1"
zlgcan = "0.3"
tokio = { version = "1", features = ["full"] }
```

### Exemple de code

```rust
use rs_can::{CanDevice, CanFrame, ChannelConfig, DeviceBuilder};
use zlgcan::{
    can::{ZCanChlMode, ZCanChlType},
    device::ZCanDeviceType,
    driver::ZDriver,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = DeviceBuilder::new();

    let mut ch_cfg = ChannelConfig::new(500_000);
    ch_cfg
        .add_other("channel_mode", Box::new(ZCanChlMode::Normal))
        .add_other("channel_type", Box::new(ZCanChlType::CAN));

    builder
        .add_other("library_path", Box::new("zlgcan.dll".to_string()))
        .add_other("device_type", Box::new(ZCanDeviceType::ZCAN_USBCANFD_200U))
        .add_other("device_index", Box::new(0u32))
        .add_config(0, ch_cfg);

    let device = builder.build::<ZDriver>()?;

    // Envoi
    let frame = CanFrame::new(0x123, &[0x01, 0x02, 0x03])?;
    device.transmit(0, &frame)?;

    // Réception
    let frames = device.receive(0, 10)?;
    for f in frames {
        println!("Received: {:?}", f);
    }

    Ok(())
}
```

### Prérequis

1. **Driver ZLG** : Télécharger depuis le site officiel ZLG ou le CD fourni
2. **DLL** : `zlgcan.dll` doit être accessible

### Avantages

- ✅ **Support async natif** (tokio compatible)
- ✅ **Cross-platform** Windows + Linux
- ✅ **Hardware abordable** (~€35-50 pour basique)
- ✅ Support CAN FD (modèles FD)
- ✅ Support UDS protocol

### Inconvénients

- ⚠️ **Hardware de niche** - difficile à trouver en Europe
- ⚠️ Documentation principalement en chinois
- ⚠️ Qualité variable des clones sur Aliexpress
- ⚠️ Support/SAV limité hors de Chine

### Verdict

**Bonne option budget** si vous trouvez un adaptateur ZLG fiable. Le support async est un gros plus.

---

## 🔴 Solution 6 : Créer une couche d'abstraction complète

**Effort** : Élevé | **Risque** : Élevé | **Temps** : 1-2 mois

Créer une abstraction CAN complète inspirée du trait `embedded-can`.

### Architecture proposée

```
oze-canopen-viewer
├── can-abstraction/       (nouveau crate)
│   ├── src/
│   │   ├── lib.rs         (traits communs)
│   │   ├── linux.rs       (backend socketcan)
│   │   ├── windows.rs     (backend pcan)
│   │   └── mock.rs        (backend mock pour tests)
│   └── Cargo.toml
├── oze-canopen/           (fork modifié)
│   └── ... utilise can-abstraction
└── src/                   (inchangé)
```

### Traits à définir

```rust
pub trait CanAdapter: Send + Sync {
    fn open(interface: &str, bitrate: u32) -> Result<Self, CanError>;
    fn send(&mut self, frame: &CanFrame) -> Result<(), CanError>;
    fn recv(&mut self) -> Result<CanFrame, CanError>;
    fn recv_timeout(&mut self, timeout: Duration) -> Result<Option<CanFrame>, CanError>;
    fn close(&mut self) -> Result<(), CanError>;
}

pub trait AsyncCanAdapter: Send + Sync {
    async fn send(&mut self, frame: &CanFrame) -> Result<(), CanError>;
    async fn recv(&mut self) -> Result<CanFrame, CanError>;
}
```

---

## 🔵 Solution 6 : Backend réseau (CAN over TCP/IP)

**Effort** : Moyen | **Risque** : Faible | **Temps** : 1-2 semaines

Créer un bridge réseau entre une machine Linux avec CAN et l'application Windows.

### Architecture

```
[Windows PC]                    [Linux PC/Raspberry Pi]
oze-canopen-viewer  ←→ TCP/IP ←→  can-bridge → CAN bus
```

### Implémentation

1. **Serveur Linux** (nouveau composant) :

   ```rust
   // Lit les frames CAN et les envoie via TCP
   async fn can_to_tcp(can: CanSocket, tcp: TcpStream) {
       loop {
           let frame = can.recv().await?;
           tcp.write_all(&serialize(frame)).await?;
       }
   }
   ```

2. **Client Windows** (nouveau backend) :

   ```rust
   struct TcpCanBackend {
       stream: TcpStream,
   }

   impl CanBus for TcpCanBackend {
       fn recv(&self) -> Result<CanFrame, Error> {
           // Lire depuis TCP
       }
   }
   ```

**Avantages** :

- Aucune modification des dépendances Linux
- Fonctionne avec n'importe quel hardware CAN côté Linux
- Peut utiliser un Raspberry Pi comme bridge

**Inconvénients** :

- Latence réseau ajoutée
- Nécessite deux machines
- Plus complexe à déployer

---

## Comparaison des solutions pour Windows natif

| Solution           | Effort    | Async | Hardware       | Prix HW               | Maturité  | Notes                     |
| ------------------ | --------- | ----- | -------------- | --------------------- | --------- | ------------------------- |
| **SH-C30A (PCAN)** | 🟡 Moyen  | ❌    | SH-C30A flashé | **€0** (déjà possédé) | 🟡 Stable | ⭐ Adaptateur existant    |
| **automotive**     | 🟢 Moyen  | ✅    | comma.ai Panda | **$99**               | 🟢 Stable | ⭐ Meilleure option async |
| host-can           | 🟡 Moyen  | ❌    | PCAN-USB       | €195-347              | 🟡 Early  | Compatible SH-C30A PCAN   |
| peak-can-sys       | 🔴 Élevé  | ❌    | PCAN-USB       | €195-347              | 🟡 Stable | Compatible SH-C30A PCAN   |
| zlgcan             | 🟡 Moyen  | ✅    | ZLG USBCAN     | €35-200               | 🟡 Stable |                           |
| SH-C30A (WSL2 USB) | 🟢 Faible | ✅    | SH-C30A        | **€0**                | 🟢 Stable | Nécessite WSL2            |
| Abstraction        | 🔴 Élevé  | ?     | Variable       | Variable              | N/A       |                           |
| Réseau             | 🟡 Moyen  | ✅    | Linux + réseau | ~€50 (RPi)            | N/A       |                           |

### Légende

- **Async** : Compatible avec tokio (important car oze-canopen utilise tokio)
- **Maturité** : Stabilité des APIs

---

## Recommandations

### 🏆 Pour votre SH-C30A existant (RECOMMANDÉ)

→ **Solution 1.5 Option A (SH-C30A + PCAN firmware)** :

- ✅ **Vous avez déjà le hardware** (€0 supplémentaire)
- ✅ Flasher une fois avec firmware PCAN
- ✅ Utiliser `host-can` ou `peak-can-sys`
- ✅ .exe Windows natif fonctionnel
- ⚠️ Pas d'API async (mais fonctionnel)

**Alternative rapide** : **Solution 1.5 Option D (WSL2 USB passthrough)**

- ✅ Pas besoin de flasher
- ✅ Fonctionne avec code existant
- ⚠️ Nécessite WSL2 sur chaque machine

### 🏆 Pour un .exe Windows natif avec async (SI ACHAT HARDWARE)

→ **Solution 2 (`automotive` + Panda)** :

- API async compatible tokio ✅
- Cross-platform (Linux/Windows/macOS) ✅
- Hardware abordable ($99) ✅
- Bien maintenu et documenté ✅

### Pour du hardware PCAN existant (autre que SH-C30A)

→ **Solution 3 (`host-can`)** : Si vous avez déjà un PCAN-USB

### Pour un budget serré

→ **Solution 5 (`zlgcan`)** : Adaptateurs ZLG ~€35-50

### Pour un prototype sans achat hardware

→ **Solution 1 (WSL2)** : Fonctionne avec vcan

### Pour un déploiement distribué

→ **Solution 7 (Réseau)** : Raspberry Pi + bridge CAN

---

## Plan d'action recommandé

### 🎯 Plan spécifique pour SH-C30A avec firmware PCAN (VOTRE CAS)

**✅ Avantage** : Vous avez déjà le firmware PCAN installé !

#### Étapes immédiates

1. **Installer PCAN-Basic sur Windows** :

   - Télécharger : https://www.peak-system.com/Downloads.76.0.html
   - Installer le driver
   - Brancher le SH-C30A
   - Vérifier dans le Gestionnaire de périphériques qu'il apparaît comme périphérique PCAN

2. **Tester la détection** :

   ```powershell
   # Sur Windows PowerShell
   # L'adaptateur devrait être visible comme PCAN_USBBUS1
   ```

3. **Choisir la solution Rust** :

   - **Option A** : `host-can` (plus simple, abstraction déjà faite)
   - **Option B** : `peak-can-sys` (plus de contrôle, mais plus de code)

4. **Modifier oze-canopen** :

   - Forker le projet `oze-canopen` (ou créer une branche locale)
   - Remplacer `socketcan` par `host-can` avec features conditionnelles
   - Adapter le code pour utiliser `host-can::Adapter`

5. **Compiler pour Windows** :

   ```bash
   rustup target add x86_64-pc-windows-gnu
   # Ou pour MSVC
   rustup target add x86_64-pc-windows-msvc

   cargo build --release --target x86_64-pc-windows-gnu
   ```

#### Configuration Cargo.toml pour oze-canopen

```toml
[target.'cfg(target_os = "linux")'.dependencies]
host-can = { version = "0.1", features = ["socketcan"] }

[target.'cfg(target_os = "windows")'.dependencies]
host-can = { version = "0.1", features = ["pcan"] }
```

#### Option B : WSL2 USB passthrough (garder Candlelight)

1. **Configurer WSL2** :

   ```powershell
   # Sur Windows
   wsl --install
   ```

2. **Installer USB passthrough** :

   ```powershell
   # Installer usbipd
   winget install usbipd
   ```

3. **Attacher le SH-C30A à WSL2** :

   ```powershell
   usbipd wsl list
   usbipd wsl attach --busid <BUSID>
   ```

4. **Dans WSL2, configurer CAN** :

   ```bash
   sudo modprobe can can-raw slcan
   sudo slcan_attach -f -s6 -o /dev/ttyACM0
   sudo slcand ttyACM0 can0
   sudo ip link set can0 up type can bitrate 500000
   ```

5. **Compiler et exécuter** :
   ```bash
   cargo build --release
   ./target/release/oze-canopen-viewer --can can0 --bitrate 500000
   ```

### Plan général (si achat nouveau hardware)

#### Étape 1 : Acheter le hardware

- **Option A** : [comma.ai Panda](https://comma.ai/shop/products/panda-obd-ii-dongle) - $99 + $30 shipping
- **Option B** : [PCAN-USB](https://www.peak-system.com/) - €195+

#### Étape 2 : Forker et modifier `oze-canopen`

Remplacer la dépendance `socketcan` par une abstraction :

```toml
# Nouveau Cargo.toml de oze-canopen
[dependencies]
automotive = "0.2"  # Pour la solution Panda
# OU
host-can = { version = "0.1", features = ["socketcan", "pcan"] }
```

#### Étape 3 : Adapter le code

Modifier les appels CAN pour utiliser la nouvelle abstraction.

#### Étape 4 : Cross-compiler

```bash
# Depuis Linux
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

---

## Ressources

### Crates Rust

- [automotive](https://docs.rs/automotive) - **Recommandé**
- [host-can](https://docs.rs/host-can)
- [peak-can-sys](https://docs.rs/peak-can-sys)
- [zlgcan](https://docs.rs/zlgcan)
- [embedded-can traits](https://docs.rs/embedded-can)

### Hardware

- [comma.ai Panda](https://comma.ai/shop/products/panda-obd-ii-dongle) - $99
- [PCAN-USB (Peak System)](https://www.peak-system.com/Downloads.76.0.html) - €195+
- [ZLG USBCAN](https://www.zlg.cn/) - Variable

### Documentation

- [WSL2 SocketCAN Setup](https://eclipse-openbsw.github.io/openbsw/sphinx_docs/doc/learning/setup/setup_wsl_socketcan.html)
- [PCAN-Basic API](https://www.peak-system.com/produktcd/Develop/PC%20interfaces/Windows/PCAN-Basic%20API/)
