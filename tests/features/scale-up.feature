Feature: Scale-up

  Background:
    Given drive target is online
    And drive disk type is Gp2
    And drive max bsu count is 10
    And drive max total size is unlimited
    And drive initial size is 10Gib
    And drive max used space is 85%
    And drive min used space is 20%
    And drive scale factor is 20%
    And reconcile runs
    And drive is mounted
    And drive has 1 BSU
    And drive size is 10Gib

  Scenario: Writing 50% of disk capacity don't trigger scale-up
    Given drive usage is 5Gib
    When reconcile runs
    Then drive has 1 BSU
    And drive size is 10Gib
    And cleanup

  Scenario: Writing 90% of disk capacity trigger scale-up
    Given drive usage is 9Gib
    When reconcile runs
    Then drive has 2 BSU
    And drive size is 22Gib
    And cleanup

# different feature ?
# TODO: test limit -> total size
# TODO: test limit -> max disks