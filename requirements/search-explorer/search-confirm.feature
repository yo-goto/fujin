Feature: 検索確定後の状態

  Scenario: Enterでジャンプするとnavモードごと抜ける
    Given 検索サブモード中で絞り込み結果でカーソルを合わせている
    When ユーザーが Enter キーを押す
    Then キーの横取りが解除される
    And 選択行のペインへフォーカスが移る
    And ツリー表示に戻る

  Scenario: 再入場時にクエリは残らない
    Given 検索サブモードを抜けた直後である
    When ユーザーが再び "/" キーを押す
    Then クエリは空の状態で始まる

  Scenario: 0件ヒット時のEnterは何もしない
    Given 検索サブモード中でクエリに一致するペインが1件もない
    When ユーザーが Enter キーを押す
    Then ジャンプは起きない
    And 検索サブモードに留まる
