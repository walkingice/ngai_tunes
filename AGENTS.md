# Summary

- 專案名稱：ngai_tunes，透過 TUI 操縱的音樂播放程式
- 可播放 mp3 與 flac 檔，能在 Linux 與 Mac 執行
- 可讀取 mp3 tag 與 flac 的資訊
- 使用 Rust 實作，提供豐富快速鍵
- client-server 架構
    - detach 並關閉介面後，仍然可以播放音樂。隨時可以重新 attach daemon 繼續操作 playback
    - client 啟動後，如果沒有 daemon 則啟動一個，如果有存在的 daemon 則連上

## Spec

- sock 檔案寫在 `/tmp/ngai_tunes/daemon.sock`
- 其他檔案都寫在 `~/.config/ngai_tunes/` 裡面
- 目前沒有瀏覽檔案的功能，啟動 client 時直接搜尋指定路徑裡面支援的音樂檔案，列入 Playing tab，
    - 執行程式時可以指定路徑，若沒有指定路徑則用 PWD 目錄

## Architecture

- single binary，透過啟動的參數決定是 client 或是 server mode。預設就是 client mode
- 不同的 module 會放在不同的目錄，在 project root directory 進行編譯，得到 binary `ngai_tunes`
- 以 client-server 架構
    - 不依賴 systemd 或 launchd
    - flock() + Unix Domain Socket
    - 用 JSON line 通訊
- server/daemon module
    - 最多只會有一個 daemon 在執行
    - 負責 playback，並且回應 client 對於資訊的查詢
    - 接受 client 傳來的 files list 來播放，如果要播放時發現某個檔案已經不存在，跳過該檔案。如果所有檔案都不存在，停止播放
    - 接受 client 傳來的指令，進行播放、暫停，快轉或是切換曲目
    - client 如果傳來 `exit_on_stopping`，若沒有音樂在播放的話則終止 daemon
- TUI client module
    - client 啟動時檢查有沒有 running server
        - 若有則連上並詢問狀態
        - 若無，則在真正需要跟 server 互動時再啟動新的 server
    - 提供介面給 User 操作
    - 送出檔案清單以及 metadata 告知 server 有哪些檔案要播放
    - 可讀取檔案的 metadata
- playback module
    - 專注在播放音樂
    - 有 Mac OS X 與 Linux 的不同實作，有相同抽象介面，但真正的實作會放在不同的目錄底下
- @docs/DAEMON_ARCH.md 是 daemon 的實作建議
- @docs/PLAYBACK_ARCH.md 是 playback 的實作建議
- @docs/UI.md 是 TUI Client 的實作建議

## Server 狀態

- Playing - 正在播放
- Pausing - 播放但是暫停
    - Pausing 到 Playing 代表該曲目接續播放
- Stopping - 沒有播放任何音樂
    - Stopping 到 Playing 代表該曲目從頭播放

## Features plan

未來會支援的功能，但目前尚未規劃實作與介面

- 簡易編輯 mp3 tag
- 編輯或讀取 playlist
- 簡易的 GUI client，在 Mac or Linux 上有不同實作
- 多語系，目前只有英文

