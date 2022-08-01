# Archive decentralization

## Abstract

## Subsquide team goals

## Similar approaches

### POKT Network

### The Graph

Graph Protocol falls into a category as a layer 2 read-scalability solution.

#### High-Level Architecture

```ascii
    +------------------------------------------------------------------+
    |                                                                  |
    | Decentralized Application (dApp)                                 |
    |                                                                  |
    +-+---------------------------------^--------------------------+---+
      |                                 |                          |
      |                              Queries                       |
      |                                 |                          |
      |   +-----------------------------+--------------------+     |
      |   |                                                  |     |
      |   |  Query Nodes and Clients                         | Micropayments
      |   |                                                  |     |
      |   +---------+-------------------^--------------------+     |
      |             |                   |                          |
Transactions   Attestations     (Reads, Attestations)              |
      |             |                   |                          |
      |   +---------v-----------+  +----+--------------------------v---+
      |   |                     |  |                                   |
      |   |  Fisherman Service  |  | Indexing Nodes                    |
      |   |                     |  |                                   |
      |   +---------+-----------+  +----^-------------------^----------+
      |             |                   |                   |
      |         Disputes          (Events, Data)          Data
      |             |                   |                   |
    +-v-------------v-------------------+-----+ +-----------+----------+
    |                                         | |                      |
    |                Ethereum                 | |   IPFS               |
    |                                         | |                      |
    +-----------------------------------------+ +----------------------+
```

**Query [Nodes | Clients]**

Query Nodes provide an abstraction on top of the low-level read API provided by the Indexing Nodes. In addition to providing an interface to dApps, the Query Node is responsible for discovering Indexing Nodes in the network that are indexing a specific dataset, and selecting an Indexing Node to read from based on factors such as price and performance (see Query Processing). It may also optionally forward attestations along to a Fisherman Service.

**Indexing nodes**

Indexing Nodes index one or more user-defined datasets, called subgraphs. These nodes perform a deterministic streaming extract, transform and load (ETL) of events emitted by the Ethereum blockchain. These events are processed by user-defined logic called mappings which run deterministically inside a WASM runtime, and are also able to load additional data from the Ethereum blockchain or IPFS, in order to compute the current state of a subgraph.

**Fisherman service**

Fisherman Services accept read responses and attestations which they may verify, and in the event of an invalid response, may file a protocol-level dispute.

#### Mechanism design

Indexing Node operators stake deposits of Graph Tokens for particular datasets, called subgraphs, to gain the right to participate in the data retrieval marketplaces for that dataset-indexing data and responding to read requests in exchange for micropayments. This deposit is forfeit in the event that the work is not performed correctly, or is performed maliciously, as defined in the slashing conditions.

**Graph Token**

Graph Tokens, which are the only token that may be used for staking in the network. However, ETH or DAI is used for paying for read operations, thus reducing friction and balance sheet risk for end-users of dApps that query The Graph.

**Staking**

Indexing Nodes deposit a `stakingAmount` of Graph Tokens to process read requests for a specific dataset, which is identified by its `subgraphID`. The stakingAmount must be in the set of the top N staking amounts, where N is determined by the maxIndexers parameter that is set via governance.

Indexing Nodes that have staked for a dataset are not limited by the protocol in how many read requests they may process for that dataset. However, it may be assumed that Indexing Nodes with higher deposits will receive more read requests and, thus, collect more fees, if all else is equal, as this represents a greater economic security margin to the end user.

**Data Retrieval Market**

Indexing Nodes which have staked to index a particular dataset, will be discoverable in the data retrieval market for that dataset.

Indexing Nodes receive requests which include a Read Operation and a Locked Transfer.
The Read Operation fully defines the data that is being requested, while the Locked Transfer is a micropayment that is paid, conditional, on the Indexing Node producing a Read Response along with a signed Attestation message which certifies the response data is correct.

**Data Retrieval Pricing**

Pricing in the data retrieval market is set according to the bandwidth and compute required to process a request.

- Compute is priced as a gasPrice, denominated in ETH or DAI, where the gas required for a request is determined by the specific read operation and parameters. See Read Interface for operation specific gas prices.
- Bandwidth is priced in bytesPrice, denominated in ETH or DAI, where bytes refers to the size of the data portion of the response, measured in bytes.

Indexing Nodes respond with their compute and bandwidth costs in response to the getPrices method in the JSON-RPC API.

**Verification**

A Fisherman Service is an economic agent who verifies read responses in exchange for a reward in cases where they detect that an Indexing Node has attested to an incorrect response, and the Fisherman successfully disputes the response on-chain.

**Curation Market**

Curators are economic agents who earn rewards by betting on the future economic value of datasets, perhaps with the benefit of private information.

A Curator stakes a deposit of Graph Tokens for a particular dataset in exchange for dataset-specific subgraph tokens. These tokens entitle the holder to a portion of a curation reward, which is paid in Graph Tokens through inflation. See Inflation Rewards for how curation reward is calculated for each dataset.

**Stake delegation**

Token holders who do not feel equipped to perform one of these functions may delegate their tokens to an Indexing Node that is staked for a particular dataset. In this case, the delegator is the residual claimant for their stake, earning participation rewards according to the activities of the delegatee Indexing Node but also forfeiting their stake in the event that the delagatee Indexing Node is slashed.

### Kyve

### Subquery

### Summary

## Proposed design
