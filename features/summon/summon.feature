Feature: fujin が居ないタブでの臨時召喚

  Background:
    Given セッションのどこかのタブに常駐の fujin が居る

  Scenario: 居ないタブで呼び出す
    Given フォーカス中のタブに fujin が居ない
    When ユーザーがnavモードの入場キーを押す
    Then そのタブにフローティングの fujin が現れる
    And 常駐サイドバーと同じ位置・同じ幅で表示される
    And そのまま navモードに入っていて、j や k で選択を動かせる

  Scenario: 居るタブでは召喚しない
    Given フォーカス中のタブに常駐の fujin が居る
    When ユーザーがnavモードの入場キーを押す
    Then 常駐の fujin が navモードに入る
    But フローティングは現れない

  Scenario: セッションを開いた直後でも呼び出せる
    Given セッションを開いてから、まだ fujin の居るタブを一度も見ていない
    And フォーカス中のタブに fujin が居ない
    When ユーザーがnavモードの入場キーを押す
    Then フローティングの fujin が現れる

  Scenario: 同じキーで引っ込める
    Given navモードの入場キーで fujin が召喚されている
    When ユーザーがnavモードの入場キーをもう一度押す
    Then 召喚インスタンスのペインが閉じる
    And そのタブに fujin のペインは残らない

  Scenario: 何度押しても増えない
    Given navモードの入場キーで fujin が召喚されている
    When ユーザーがnavモードの入場キーを続けて何度も押す
    Then そのタブの召喚インスタンスは常に0個か1個である

  Scenario: navモードから抜けたら閉じる
    Given 召喚インスタンスが navモードに入っている
    When ユーザーが Esc キーを押す
    Then navモードを抜ける
    And 召喚インスタンスのペインが閉じる

  Scenario: ジャンプしたら用は済んでいる
    Given 召喚インスタンスが navモードに入っている
    When ユーザーが選択中のペインへジャンプする
    Then そのペインにフォーカスが移る
    And 召喚インスタンスのペインが閉じる

  Scenario: プラグインの状態に依らず手で消せる
    Given 何らかの理由で召喚インスタンスが取り残されている
    When ユーザーがそのペインを通常のペイン操作で閉じる
    Then そのペインが閉じる
    But 常駐サイドバーは閉じない

  Scenario: 取り残しをまとめて掃除する
    Given 何らかの理由で召喚インスタンスが複数取り残されている
    When ユーザーが fujin_dismiss pipe を送る
    Then そのセッションの召喚インスタンスがすべて閉じる
    But 常駐サイドバーは閉じない
