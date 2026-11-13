# Universal Remotion Studio

Aplikasi desktop Windows yang ringan untuk mengelola proyek Remotion: impor ZIP
(misalnya hasil AI coding tool), inspeksi, preview, dan render — tanpa mengubah
versi dependensi proyek dan tanpa menumpuk file sementara di komputer.

Aplikasi ini hanya orkestrator + UI. Mesin render tetap Remotion milik
masing-masing proyek (`package.json` + lockfile + CLI lokal proyek itu sendiri).
Spesifikasi lengkap: [docs/SPEC.md](docs/SPEC.md).

## Cara mendapatkan installer Windows

Installer dibangun otomatis oleh GitHub Actions (workflow **Build Windows**):

1. Buka tab **Actions** di repository, pilih run terbaru **Build Windows**.
2. Unduh artifact `universal-remotion-studio-windows` (berisi installer NSIS `.exe`).
3. Jalankan installer. Instalasi per-user, tidak butuh Administrator.

Kebutuhan: Windows 10/11 64-bit. WebView2 diunduh otomatis oleh installer bila belum ada.
Installer belum ditandatangani (code signing), jadi SmartScreen bisa menampilkan peringatan.

## Membangun sendiri di Windows

Prasyarat: [Rust](https://rustup.rs) (stable, MSVC), Node.js 20+, dan
"Desktop development with C++" dari Visual Studio Build Tools.

```powershell
npm ci
npm run tauri dev      # mode pengembangan
npm run tauri build    # installer di src-tauri\target\release\bundle\nsis\
```

## Alur pakai

1. **Import ZIP** — pilih / drop file `.zip`. Arsip divalidasi (path traversal,
   symlink, batas ukuran) lalu diekstrak ke workspace aplikasi. Tidak ada kode
   proyek yang dijalankan saat impor.
2. **Overview** — laporan inspeksi: versi Remotion & `@remotion/*`, React,
   kebutuhan Node, package manager, lockfile, perintah install, entry point,
   komposisi, font. Temuan *terkonfirmasi* dipisah dari *kemungkinan masalah*.
3. **Environment** — setelah persetujuan eksplisit (dengan pilihan *Safe install*
   tanpa lifecycle script), aplikasi mengunduh runtime Node terkelola dan
   menjalankan install beku sesuai lockfile (`npm ci`,
   `pnpm install --frozen-lockfile`, `yarn install --frozen-lockfile/--immutable`).
4. **Preview** — Remotion Studio dari CLI lokal proyek, port bebas, hanya localhost.
5. **Render** — komposisi, codec, skala, concurrency, rentang frame; masuk ke
   **Render Queue** (default satu render sekaligus) dengan progres, log, dan cancel.
6. **Storage** — rincian pemakaian + tombol pembersihan.

## Struktur

```text
src/                      UI React + TypeScript
  pages/ components/ hooks/ services/ types/
src-tauri/
  src/                    Shell Tauri 2 (lapisan command tipis)
  core/                   Crate `urs-core`: semua logika, tanpa dependensi UI
    src/project/          ekstraksi ZIP aman, inspeksi statis
    src/runtime.rs        runtime Node terkelola (unduh + verifikasi SHA-256)
    src/package_manager.rs rencana install berbasis lockfile
    src/remotion.rs       resolusi & argumen CLI Remotion lokal
    src/process.rs        child process: argumen array, timeout, kill seluruh tree
    src/app/              orkestrasi: projects, environment, preview, render, storage
    src/diagnostics.rs    error yang bisa ditindaklanjuti
    src/logs.rs           log per proyek, redaksi rahasia, rotasi & retensi
    tests/                uji alur offline + uji nyata (jaringan)
```

Data aplikasi: `%LOCALAPPDATA%\UniversalRemotionStudio\`
(`runtimes/`, `workspaces/<id>/{source,temp,original.zip}`, `cache/`, `logs/`,
`settings/`). Bisa dipindah dengan variabel lingkungan `URS_DATA_DIR`.
Hasil render default ke `Videos\Remotion Renders`.

## Jaminan keamanan & kebersihan

- Entri ZIP dengan `..`, path absolut, drive letter, nama device Windows, atau
  symlink ditolak/dilewati; batas ukuran dihitung dari byte yang benar-benar ditulis.
- Semua proses dijalankan dengan array argumen (bukan string shell) memakai
  runtime Node milik aplikasi. Node sistem, PATH, paket global, dan font sistem
  tidak pernah diubah.
- Setiap child process masuk ke Windows Job Object: cancel, tutup aplikasi,
  atau crash aplikasi mematikan seluruh pohon proses (browser, ffmpeg, dev server).
- Server preview dipaksa hanya mendengarkan di `127.0.0.1` (Remotion Studio
  aslinya mendengarkan di semua interface).
- Render ditulis ke file `*.partial-<id>.*`, diverifikasi tidak kosong, baru
  diganti nama. File sementara dihapus setelah sukses, gagal, maupun cancel.
- Semua penghapusan rekursif melewati satu fungsi (`fsutil::safe_remove`) yang
  menolak path di luar folder aplikasi. ZIP asli dan hasil render tidak pernah
  dihapus oleh pembersihan otomatis.
- Mode **Save Space** menghapus workspace + `node_modules` setelah render sukses,
  hanya jika ZIP masih ada.
- Saat start, aplikasi membersihkan sisa crash: folder temp, impor setengah
  jalan, unduhan parsial, dan output parsial dari job yang terputus.

## Pengujian

```bash
cd src-tauri
cargo test -p urs-core            # unit + alur offline (tool palsu)

# Uji nyata: unduh Node terkelola, install dari lockfile, preview, render
URS_E2E_ZIPS=/path/a.zip,/path/b.zip cargo test -p urs-core --test e2e_network -- --ignored --nocapture
```

CI menjalankan uji nyata tersebut di Windows dengan dua proyek fixture
(Remotion 4.0.290 dan 4.0.150) berdampingan. Di Linux, uji yang sama juga
lulus dengan Remotion 3.3.102.

## Batasan yang diketahui

- `node_modules` berada di dalam folder proyek (`workspaces/<id>/source/...`),
  karena resolusi modul Node mengharuskannya; bukan sejajar `source/` seperti
  ilustrasi di spesifikasi.
- Browser render diunduh oleh Remotion versi masing-masing proyek ke
  `node_modules/.remotion` (dihitung sebagai "Browser cache"). Berbagi satu
  browser lintas versi Remotion tidak aman, jadi tidak dipaksakan; tersedia
  opsi *Browser executable* di Settings.
- Remotion 3.x mengunduh FFmpeg dari server yang kini sudah tidak melayani
  (HTTP 403); proyek Remotion 3 butuh FFmpeg terpasang di PATH. Remotion 4
  membawa FFmpeg sendiri.
- Lockfile Bun tidak didukung (diperlakukan sebagai "tanpa lockfile", perlu izin).
- Versi Yarn Berry/pnpm ditebak dari lockfile bila `packageManager` tidak diisi.
- Font: deteksi + penambahan file ke `public/fonts`. Pemasangan font per-render
  untuk Chromium dan font library aplikasi belum ada.
- Fitur Phase 3 (snapshot environment, mode Docker, bundel diagnostik) belum ada.
