Feature: トリアージモードの入退場

  Background:
    Given navモードでサイドバーにフォーカスしている

  Scenario: navモード中にトリアージモードへ入る
    When ユーザーが "t" キーを押す
    Then サイドバーの表示がトリアージ一覧に切り替わる
    And ツリー表示は隠れる

  Scenario: Escでツリー表示へ戻る
    Given トリアージモードでサイドバーにフォーカスしている
    When ユーザーが Esc キーを押す
    Then ツリー表示に戻る
    And navモードは継続している

  Scenario: Enterでジャンプするとnavモードごと抜ける
    Given トリアージモードで一覧の1行を選択している
    When ユーザーが Enter キーを押す
    Then 選択した行に対応するペインへフォーカスが移る
    And navモードを抜ける

  Scenario: ジャンプ先ペインの既読状態がクリアされる
    Given トリアージモードで "blocked" 状態のペインを選択している
    When ユーザーが Enter キーを押す
    Then ジャンプ先のペインの状態が既読（idle）に変わる
    And 他のサイドバーインスタンスの表示にも同じ既読クリアが反映される

  Scenario: トリアージ行の行クリックでもフォーカスが移る
    Given トリアージモードで一覧が表示されている
    When ユーザーがトリアージ行を行クリックする
    Then その行に対応するペインへフォーカスが移る
    And navモードを抜ける

  Scenario: 安全弁はトリアージモードでも維持される
    Given トリアージモードでサイドバーにフォーカスしている
    When ユーザーが未定義のキー（"?" を除く）または修飾キー付きのキーを押す
    Then navモードごと抜ける

  # "?" だけは未定義キーの例外。安全弁の対象外としてヘルプオーバーレイを開く
  # （要件: ../nav-mode/nav-mode-hints.feature, triage-list-display.feature）
  Scenario: "?" は安全弁の対象外でヘルプオーバーレイを開く
    Given トリアージモードでサイドバーにフォーカスしている
    When ユーザーが "?" キーを押す
    Then サイドバーの表示がヘルプオーバーレイに切り替わる
    And navモードは継続している
