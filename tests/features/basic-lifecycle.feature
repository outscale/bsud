Feature: Basic lifecycle

  Background:
    Given drive target is online
    And drive disk type is Gp2
    And drive max bsu count is 10
    And drive max total size is unlimited
    And drive initial size is 10Gib
    And drive max used space is 85%
    And drive min used space is 20%
    And drive scale factor is 20%

  Scenario: Initial BSU should be created and mounted
    Given drive has no BSU
    When reconcile runs
    Then drive has 1 BSU
    And drive is mounted
    And drive size is 10Gib
    And cleanup