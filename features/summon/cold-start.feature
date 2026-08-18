Feature: fujin が1つも居ないセッションでの呼び出し

  Background:
    Given セッションのどのタブにも fujin が居ない

  Scenario: 入場キーで召喚インスタンスが開く
    When ユーザーがnavモードの入場キーを押す
    Then そのタブに fujin がフローティングで現れる
    And 常駐サイドバーと同じ位置・同じ幅（左端・全高）で表示される
    And そのまま navモードに入っていて、j や k で選択を動かせる

  Scenario: 抜けたら閉じる
    Given 上記の召喚インスタンスが開いている
    When ユーザーが Esc キーを押す
    Then 召喚インスタンスのペインが閉じる

  Scenario: ジャンプしたら閉じる
    Given 上記の召喚インスタンスが開いている
    When ユーザーが選択中のペインへジャンプする
    Then そのペインにフォーカスが移る
    And 召喚インスタンスのペインが閉じる

  Scenario: レイアウトで常駐している fujin は臨時扱いしない
    Given レイアウトによってタイルで常駐している fujin がある
    When ユーザーがnavモードの入場キーを押して navモードを抜ける
    Then 常駐サイドバーは閉じない
    And 位置も幅も変わらない
