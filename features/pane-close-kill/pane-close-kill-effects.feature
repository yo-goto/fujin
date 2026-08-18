Feature: close/kill/kill→closeそれぞれの効果

  Scenario: closeはプロセスに触れずペインを閉じる
    Given 選択行のペインでプロセスが動いている
    When そのペインに対して close が実行される
    Then そのペインは閉じる
    And プロセスへシグナルは送られない

  Scenario: killは素のシェルペインで自動的にペインも閉じる
    Given 選択行のペインが素のシェルペインで、対話的に起動したプロセスが動いている
    When そのペインに対して kill が実行される
    Then そのプロセスは終了する
    And そのペインも自動的に閉じる

  Scenario: killはコマンドペインではペインを閉じない
    Given 選択行のペインがコマンドペインで、コマンドが走っている
    When そのペインに対して kill が実行される
    Then そのコマンドは終了する
    And そのペインは閉じずに残る

  Scenario: kill→closeはコマンドペインでも確実にペインを閉じる
    Given 選択行のペインがコマンドペインで、コマンドが走っている
    When そのペインに対して kill→close が実行される
    Then そのコマンドは終了する
    And そのペインも閉じる

  Scenario: 対象プロセスが存在しない場合のkillは何も起きない
    Given 選択行のペインがコマンド状態を持つコマンドペインで、コマンドは既に終了している
    When そのペインに対して kill が実行される
    Then エラーにはならない
    And そのペインは閉じずに残る
