Feature: 終了操作サブモードへの入退場と種別選択

  Scenario: 終了操作の入場キーで終了操作サブモードに入る
    Given navモードで選択行がある
    When ユーザーが終了操作の入場キーを押す
    Then 終了操作サブモードに入る
    And フッターが確認プロンプトに切り替わる

  Scenario: フッターの確認プロンプトは警告色で表示される
    Given 終了操作サブモードに入っている
    Then フッターは警告色で表示される
    And 選択行のペイン名はフッターに表示されない

  Scenario: closeキーでcloseを実行する
    Given 終了操作サブモードに入っている
    When ユーザーが close のキーを押す
    Then 選択行のペインに対して close が実行される
    And 終了操作サブモードを抜けてnavモードに戻る

  Scenario: killキーでkillを実行する
    Given 終了操作サブモードに入っている
    When ユーザーが kill のキーを押す
    Then 選択行のペインに対して kill が実行される
    And 終了操作サブモードを抜けてnavモードに戻る

  Scenario: kill→closeキーでkill→closeを実行する
    Given 終了操作サブモードに入っている
    When ユーザーが kill→close のキーを押す
    Then 選択行のペインに対して kill→close が実行される
    And 終了操作サブモードを抜けてnavモードに戻る

  Scenario: Escで取り消す
    Given 終了操作サブモードに入っている
    When ユーザーが Esc キーを押す
    Then 何も実行されない
    And 終了操作サブモードを抜けてnavモードに戻る
    And フッターは元の表示に戻る

  Scenario Outline: 対象種別を問わず3操作とも選べる
    Given "<対象種別>" のペインが選択行にある
    When ユーザーが終了操作の入場キーを押す
    Then close・kill・kill→closeのいずれも選べる

    Examples:
      | 対象種別                          |
      | エージェント状態を持つペイン           |
      | コマンド状態を持つペイン（走行中）      |
      | コマンド状態を持つペイン（終了済み）     |
      | どちらの状態も持たないペイン           |
