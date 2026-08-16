Feature: プラグインペインの表示名

  Scenario: ペイン名がfujinになる
    Given fujinプラグインが起動し権限が承認されている
    Then そのペインの表示名は "fujin" になる
    And wasmのロード元のフルURLは表示されない
