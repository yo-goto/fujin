Feature: エージェント状態のアイコン表示

  Scenario Outline: フックの通知に応じて状態アイコンが変わる
    Given "<フック>" 通知を受け取ったペインがある
    Then そのペイン行には "<状態>" を表すアイコンが表示される

    Examples:
      | フック            | 状態     |
      | UserPromptSubmit | working |
      | Notification（permission_prompt / agent_needs_input / idle_prompt） | blocked |
      | Stop             | done    |
      | SessionStart     | idle    |
      | StopFailure      | error   |

  Scenario: ツール単体の失敗では状態が変わらない
    Given "working" の状態を表示しているペインがある
    When そのペインで PostToolUseFailure 通知を受け取る
    Then そのペインの状態は "working" のまま変わらない

  Scenario: エラーで終わったターンは完了として上書きされない
    Given "error" の状態を表示しているペインがある
    When そのペインで Stop 通知を受け取る
    Then そのペインの状態は "error" のまま変わらない

  Scenario: 状態ごとに異なる色で判別できる
    Given 複数の状態のペインが同時に表示されている
    Then 状態アイコンの色は状態ごとに異なり、色だけで状態を判別できる

  Scenario: サブエージェントの活動が分かる
    Given あるペインで SubagentStart 通知を受け取った
    Then そのペイン行にサブエージェントが活動中であることが表示される

  Scenario: サブエージェント終了で表示が消える
    Given サブエージェントが活動中と表示されているペインがある
    When そのペインで SubagentStop 通知を受け取る
    Then サブエージェントの活動表示が消える

  Scenario: 未完了タスクの状態が分かる
    Given あるペインで TaskCreated 通知を受け取った
    Then そのペイン行に未完了タスクがあることが表示される

  Scenario: フックを設定していないペインは状態を持たない
    Given 一度もフックの通知を受け取っていないシェルペインがある
    Then そのペイン行には状態アイコンが表示されない
    And 状態アイコンの位置には未起動マーカーが表示される

  Scenario: 未起動マーカーは状態と見分けられる
    Given 一度もフックの通知を受け取っていないシェルペインがある
    Then 未起動マーカーには状態を表す色が付かない

  Scenario: 状態を受け取ると未起動マーカーは状態アイコンに置き換わる
    Given 未起動マーカーが表示されているペインがある
    When そのペインで状態の通知を受け取る
    Then そのペイン行には受け取った状態のアイコンが表示される
    And 未起動マーカーは表示されなくなる
