# Example

介面的示意如下

```
   +--------+--------------------------------------+
   |        |                                      |
   |        |                                      |
   |        |                                      |
   |        |                                      |
   |  Side  |              Main                    |
   |  Bar   |                                      |
   |        |                                      |
   |        |                                      |
   |        |                                      |
   |        |                                      |
   |        |                                      |
   |        |                                      |
   +--------+--------------------------------------+
   |                                               |
   |                    Playback                   |
   |                                               |
   +-----------------------------------------------+
   |                      Info                     |
   +-----------------------------------------------+
   |                     Hotkey                    |
   +-----------------------------------------------+
```

- Sidebar
    - 顯示有哪些 tab 可以切換，切換 Tab 時就會更動 Main 的畫面
    - 目前只有 `Playing` 一個 tab
    - Playing
        - 打開 client 時預設都進入這個 tab
        - 列出可供播放的音樂，可以用游標鍵上下移動，正在 focus 的項目背景色會不同
        - 正在播放的曲目會有不同顏色的字

- 可以用 Tab 鍵切換 focus，在 Sidebar, Main, Playback 之間移動
- Playback
    - 顯示正在播放的曲目，以及 progress bar
    - 可以控制：Forward, Backward, Play/Pause, Repeat(state for None or All or 1), Random or Sequential.
- Info - 用來顯示文字訊息的區塊
- Hotkey - 在最底下，提示 User 當下有哪些 Hotkey 可以使用

## Hotkey

- Main
    - Playing tab
        - Enter 鍵：將 Playing 清單送給 Server，並且指定從頭播放正在 focus 的某一首歌
        - 空白鍵: 切換 Pausing 或 Playing 狀態。
        - q 鍵：跳出 Modal，詢問 User 是否要停止 Playback 並且完全關閉 app
            - 若 User 同意，則先送出 stop 給 server，再送出 `exit_on_stopping`
        - esc：結束 client，僅送出 `exit_on_stopping`
- Playback
    - 空白鍵: 切換 Pausing 或 Playing 狀態。
    - Enter: 切換 Stopping 或 Playing 狀態。
    - N: 跳到下一首
    - P: 跳到上一首
    - R: 切換 Repeat 狀態

