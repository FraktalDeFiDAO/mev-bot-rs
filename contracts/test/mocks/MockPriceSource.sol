// SPDX-License-Identifier: UNLICENSED
pragma solidity 0.8.28;

/// Test-only Chainlink-compatible price source. Its runtime code is installed over a real source
/// on an anvil fork (anvil_setCode); the answer lives in storage slot 0 (anvil_setStorageAt), so
/// E2E tests can move prices without touching protocol code.
contract MockPriceSource {
    int256 internal answer; // slot 0

    function latestAnswer() external view returns (int256) {
        return answer;
    }

    function latestRoundData() external view returns (uint80, int256, uint256, uint256, uint80) {
        return (1, answer, block.timestamp, block.timestamp, 1);
    }

    function decimals() external pure returns (uint8) {
        return 8;
    }
}
