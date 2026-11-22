# ngai_tunes

[English](README.md) · 繁體中文

ngai_tunes 是一款適用於 Linux 和 macOS 的鍵盤操作式 MP3 和 FLAC 播放器。
終端機與選單用戶端共用同一個本機 daemon，因此關閉終端機後，音樂仍可繼續播放。

## 編譯

請在專案根目錄使用 Make、Rust 和 Cargo 執行下列指令。Linux 需要安裝
`pkg-config` 與 ALSA 開發用標頭檔（Debian 和 Ubuntu 使用 `libasound2-dev`，
Fedora 使用 `alsa-lib-devel`）。macOS 使用 Core Audio。

執行 `make` 即可列出可用的目標，輸出與 `make help` 相同。

| 指令 | 用途 |
| --- | --- |
| `make build` | 編譯 release 版，執行檔位於 `target/release/ngai-tunes`。 |
| `make debug` | 編譯 debug 版，執行檔位於 `target/debug/ngai-tunes`。 |
| `make menu` | 打包 macOS app 或編譯 Linux release 版。設定與啟動方式請見[選單用戶端](#選單用戶端)。 |
| `make test` | 執行測試。 |
| `make check` | 檢查所有 Cargo 目標。 |
| `make fmt` | 格式化 Rust 程式碼。 |
| `make fmt-check` | 檢查 Rust 程式碼格式。 |

上述執行檔路徑以 Cargo 的預設輸出目錄為準。

## 使用方式

```sh
./target/release/ngai-tunes               # 連線到目前的播放佇列
./target/release/ngai-tunes --attach      # 明確指定連線到目前的播放佇列
./target/release/ngai-tunes .             # 掃描目前的工作目錄
./target/release/ngai-tunes ~/Music/Jazz  # 掃描指定目錄
./target/release/ngai-tunes --daemon      # 在前景執行 daemon
./target/release/ngai-tunes --menu        # 開啟原生選單的播放控制
```

指定目錄時會開啟 Browsing。未指定目錄時，如果設定目錄內有 `database.jsonl`，
就會開啟 Scanned，否則會開啟 Playing。Playing 會顯示 daemon 的播放佇列。
如果 daemon 未執行，佇列會是空的。此時若未指定目錄啟動，畫面底部也會顯示提示。

要開始播放，請先指定一個目錄。在 Browsing 分頁用方向鍵選取曲目，按 Enter 將
清單載入 Playing，並從選取的曲目開始播放。按空白鍵可暫停或繼續播放。
按 Esc 可離開用戶端，並要求已手動停止播放的 daemon 結束。播放中或暫停時按
`q` 會停止播放。停止後再按 `q` 會關閉用戶端並結束 daemon。
關閉重複播放時，daemon 會在播完佇列後結束。

在 Playing 中，播放或暫停時可用左右方向鍵快轉或倒轉五秒。Vim 按鍵
`h`、`j`、`k`、`l` 分別對應左、下、上、右。按 Tab 可在主區域和 Playback
之間切換焦點。在 Playback 按 Enter 可停止播放，或從頭播放保留的曲目。
按大寫 `P` 可將焦點移到 Playback，按大寫 `S` 可移到目前顯示的歌曲清單。

按 `n` 播放下一首，按 `N` 或 `p` 播放上一首。左右方向鍵可快轉或倒轉五秒。
按 `r` 循環切換重複模式，按 `o` 切換播放順序。單曲重複會在曲目自然播完時
生效。手動切換上一首或下一首時會依照目前的播放順序。隨機播放時，上一首會依照
本輪的隨機順序往回播放。關閉重複播放時，daemon 會在本輪結束後離開。全部重複
則會重新隨機排序並開始下一輪。

按大寫 `T` 將焦點移到 Tabs，再用上下方向鍵選取 Settings，並按 Enter 開啟。
在 Tabs 按 Tab 可回到主區域。選取目錄路徑可編輯，選取 Delete 可確認刪除，
選取 Add directory 可新增掃描目錄。選取 Scan 並確認後會重建音樂資料庫。掃描成功時，
Scanned 會重新載入。

掃描目錄儲存在 `config.json` 的 `tui.scan_directories`。TUI 結束時，會將
Scanned 中有變更的篩選器和歌曲游標位置存入 `tui.scanned`。重新開啟時，無效的
篩選位置會回到 All，無效的歌曲位置會回到第一列。

在 Scanned 中，按 Tab 可從歌曲清單依序切換至各篩選器及 Playback，再回到歌曲清單。
按大寫 `R` 可將所有篩選器重設為 `All`。

另請參閱[規格](AGENTS.md)和[架構](docs/ARCH.md)。

## 選單用戶端

選單用戶端可以獨立於 TUI 啟動。daemon 離線時，**Play** 會啟動 daemon，並播放
已儲存佇列中選取的曲目。佇列為空時，可透過 TUI 加入歌曲。**Exit** 會結束
daemon 並關閉選單用戶端。已開啟的 TUI 仍可繼續使用。

### 編譯 macOS 選單列 app

先安裝 [cargo-bundle]，再從專案根目錄執行 `make menu`。
這會編譯 release 版並打包成 `Ngai Tunes.app`。

```sh
cargo install --locked cargo-bundle --version 0.12.0
make menu
```

使用 Cargo 的預設輸出目錄時，可從 Finder 開啟 app，或執行下列指令。

```sh
open "target/release/bundle/osx/Ngai Tunes.app"
```

點選選單列上的音符圖示即可開啟播放控制。也可以在終端機執行
`cargo run --release -- --menu`，直接啟動選單用戶端。

### 執行 Linux 選單用戶端

在 Linux 上，`make menu` 會編譯 release 版。請在提供 StatusNotifierItem host
的桌面工作階段執行 `./target/release/ngai-tunes --menu`。如果程式有執行但圖示
沒有出現，可用下列指令檢查工作階段的 D-Bus 上是否有 host。

```sh
busctl --user get-property org.kde.StatusNotifierWatcher /StatusNotifierWatcher \
    org.kde.StatusNotifierWatcher IsStatusNotifierHostRegistered
```

找不到服務或回傳 `b false` 時，桌面環境需要能提供 StatusNotifierItem host 的
面板或擴充套件。

[cargo-bundle]: https://github.com/burtonageo/cargo-bundle

## 設定

設定儲存在 `~/.config/ngai_tunes/config.json`。`server` 區段儲存重複播放模式。
`tui` 區段儲存通知顯示時間、掃描目錄和 Scanned 的游標位置。若要沿用舊版設定，
請將原本在最外層的設定移到各自對應的區段。

```json
{
  "server": {
    "repeat": "Off"
  },
  "tui": {
    "info_seconds": 5,
    "warning_seconds": 5,
    "error_seconds": 10,
    "scan_directories": []
  }
}
```

省略的欄位會採用上方的預設值。通知顯示時間以秒為單位。警告以黃色顯示，錯誤則以
紅色顯示。如果設定無效或無法讀取，程式會使用預設值並在啟動時輸出錯誤訊息。

daemon 會儲存 `server.repeat` 的變更，並在啟動時還原設定。
支援的值為 `Off`、`All` 和 `One`。
