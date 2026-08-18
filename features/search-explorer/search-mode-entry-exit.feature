Feature: 検索サブモードの入退場

  Scenario: navモード外では "/" は届かない
    Given navモードに入っていない
    When ユーザーが "/" キーを押す
    Then キーはフォーカス中のペインへそのまま渡る
    And 検索サブモードには入らない

  Scenario: 編集状態のEscは操作状態へ移る（決定202608131200）
    Given 検索サブモードの編集状態でクエリを入力している
    When ユーザーが Esc キーを押す
    Then 操作状態へ移る
    And クエリは破棄されない

  Scenario: 操作状態のEscで検索を取り消す
    Given 検索サブモードの操作状態にいる
    When ユーザーが Esc キーを押す
    Then クエリが破棄される
    And ツリー表示に戻る
    And 選択は検索前の位置に戻る

  Scenario: Enterで絞り込み結果へジャンプする
    Given 検索サブモード中で絞り込み結果が表示されている
    When ユーザーが Enter キーを押す
    Then 絞り込み結果内の選択行へジャンプする
