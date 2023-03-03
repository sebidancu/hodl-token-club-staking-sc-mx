#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

const NFT: u32 = 1;

#[elrond_wasm::contract]
pub trait NftStakingSc {
    #[init]
    fn init(&self,
            nft_collection: &TokenIdentifier,
            staking_enabled: &bool,
            stake_limit: &u64,
            rewards_token: &EgldOrEsdtTokenIdentifier,
            tokens_reward_per_epoch: &BigUint) {
        require!(stake_limit > &0, "Stake limit cannot be 0!");
        require!(tokens_reward_per_epoch > &0, "Rewards token distribution per epoch cannot be 0!");

        self.nft_collection().set(nft_collection);
        self.staking_enabled().set(staking_enabled);
        self.stake_limit().set(stake_limit);
        self.rewards_token().set(rewards_token);
        self.tokens_reward_per_epoch().set(tokens_reward_per_epoch);
    }

    #[only_owner]
    #[endpoint(setStakingEnabled)]
    fn set_staking_enabled(&self, staking_enabled: bool) {
        self.staking_enabled().set(staking_enabled);
    }

    #[only_owner]
    #[endpoint(addAllowedStakers)]
    fn add_allowed_stakers(&self, allowed_stakers: MultiValueEncoded<ManagedAddress>) {
        self.allowed_stakers().extend(allowed_stakers);
    }

    #[only_owner]
    #[endpoint(removeAllowedStakers)]
    fn remove_allowed_stakers(&self, allowed_stakers: MultiValueEncoded<ManagedAddress>) {
        self.allowed_stakers().remove_all(allowed_stakers);
    }

    #[endpoint(isAllowedStaker)]
    fn is_allowed_staker(&self) -> bool {
        return self.staking_enabled().get() == true
            || self.allowed_stakers().contains(&self.blockchain().get_caller()) == true;
    }

    #[only_owner]
    #[endpoint(updateStakeLimit)]
    fn update_stake_limit(&self, stake_limit: u64) {
        require!(stake_limit > 0, "Stake limit cannot be 0!");

        self.stake_limit().set(stake_limit);
    }

    #[only_owner]
    #[endpoint(updateTokensRewardPerEpoch)]
    fn update_tokens_reward_per_epoch(&self, tokens_reward_per_epoch: BigUint) {
        require!(tokens_reward_per_epoch > 0, "Tokens reward per epoch cannot be 0!");

        let current_epoch = self.blockchain().get_block_epoch();

        for staker in self.stakers().iter() {
            if self.rewards_computation_epochs(&staker).get() < current_epoch {
                self.calculate_rewards_for_user(&staker);
            }
        }

        self.tokens_reward_per_epoch().set(tokens_reward_per_epoch);
    }

    #[payable("*")]
    #[only_owner]
    #[endpoint(supply)]
    fn supply(&self) {
        let rewards_token = self.rewards_token().get();
        let (supply_token, supply_amount) = self.call_value().egld_or_single_fungible_esdt();
        require!(supply_token == rewards_token, "Invalid rewards token supply");

        self.rewards_supply().update(|rewards_supplied| *rewards_supplied += &supply_amount);
    }

    #[only_owner]
    #[endpoint(withdrawSupply)]
    fn withdraw_supply(&self, withdraw_amount: BigUint) {
        require!(self.rewards_supply().get() > withdraw_amount, "You cannot withdraw more than the existing supply");

        self.rewards_supply().update(|rewards_supplied| *rewards_supplied -= &withdraw_amount);
        self.send().direct(&self.blockchain().get_caller(), &self.rewards_token().get(), 0, &withdraw_amount);
    }

    #[endpoint(stake)]
    #[payable("*")]
    fn stake(&self) {
        require!(self.call_value().single_esdt().token_identifier == self.nft_collection().get(), "Invalid token identifier for staking function!");
        
        self.compute_rewards();
        let caller = self.blockchain().get_caller();
        
        require!(self.staking_enabled().get() == true
          || self.allowed_stakers().contains(&caller) == true, "Staking is not allowed!");
        require!(self.stake_limit().get() > self.staked_count().get(), "Staking pool is full!");

        let nft_nonce = self.call_value().single_esdt().token_nonce;

        self.staked_count().update(|staked_count| *staked_count += 1);
        self.staked_nfts(&caller).insert(nft_nonce);
        self.stakers().insert(caller);
    }

    #[endpoint(unstake)]
    #[payable("*")]
    fn unstake(&self, token_id: TokenIdentifier, nonce: u64) {
        self.compute_rewards();

        let caller = self.blockchain().get_caller();

        require!(self.staked_nfts(&caller).contains(&nonce) == true, "Invalid NFT nonce!");

        self.staked_count().update(|staked_count| *staked_count -= 1);

        self.staked_nfts(&caller).swap_remove(&nonce);
        
        self.send().direct_esdt(&caller, &token_id, nonce, &BigUint::from(NFT));

        //if this was the last unstake (no more NFTs after this operation) we reset the computation_epochs for the user
        if self.staked_nfts(&caller).len() == 0 {
            self.rewards_computation_epochs(&caller).set(0);
            self.stakers().remove(&caller);
        }
    }

    #[endpoint(getStakingRewards)]
    fn get_staking_rewards(&self) -> BigUint {
        self.compute_rewards();

        return self.rewards(&self.blockchain().get_caller()).get();
    }

    fn calculate_rewards_for_user(&self, user: &ManagedAddress) {
        let current_epoch = self.blockchain().get_block_epoch();

        //handle no rewards_computation_epochs for the caller
        let rewards_computation_epoch = self.rewards_computation_epochs(&user);
        if rewards_computation_epoch.is_empty() || rewards_computation_epoch.get() == 0 {
            self.rewards_computation_epochs(&user).set(current_epoch);
            return;
        }

        let last_computation_epoch = rewards_computation_epoch.get();

        if current_epoch < last_computation_epoch + 1 {
            return;
        }

        let rewards_epochs = current_epoch - last_computation_epoch - 1;
        let nfts = self.staked_nfts(&user);
        let nfts_number = nfts.len() as u32;
        
        let computed_rewards: BigUint = BigUint::from(rewards_epochs)
         * self.tokens_reward_per_epoch().get()
         * BigUint::from(nfts_number);

        self.rewards(&user).update(|reward| *reward += computed_rewards);
        self.rewards_computation_epochs(&user).update(|epoch| *epoch += rewards_epochs);
    }

    fn compute_rewards(&self) {
        let caller = self.blockchain().get_caller();
        let current_epoch = self.blockchain().get_block_epoch();

        //handle no rewards_computation_epochs for the caller
        let rewards_computation_epoch = self.rewards_computation_epochs(&caller);
        if rewards_computation_epoch.is_empty() || rewards_computation_epoch.get() == 0 {
            self.rewards_computation_epochs(&caller).set(current_epoch);
            return;
        }

        let last_computation_epoch = rewards_computation_epoch.get();
        
        if current_epoch < last_computation_epoch + 1  {
            return;
        }

        let rewards_epochs = current_epoch - last_computation_epoch - 1;
        let nfts = self.staked_nfts(&caller);
        let nfts_number: u32 = nfts.len() as u32;

        let computed_rewards: BigUint = BigUint::from(rewards_epochs)
         * self.tokens_reward_per_epoch().get()
         * BigUint::from(nfts_number);

        self.rewards(&caller).update(|reward| *reward += computed_rewards);
        self.rewards_computation_epochs(&caller).update(|epoch| *epoch += rewards_epochs);
    }

    #[endpoint(claim)]
    fn claim(&self) {
        self.compute_rewards();

        let rewards = self.rewards(&self.blockchain().get_caller()).get();

        require!(rewards > BigUint::zero(), "No claimable rewards");
        require!(self.rewards_supply().get() >= rewards, "Claim disabled, insufficient supply!");

        let caller = self.blockchain().get_caller();
        self.rewards_supply().update(|rewards_supplied| *rewards_supplied -= &rewards);
        self.rewards(&caller).set(BigUint::zero());
        self.send().direct(&caller, &self.rewards_token().get(), 0, &rewards);
    }

    #[view(getStakedCount)]
    #[storage_mapper("staked_count")]
    fn staked_count(&self) -> SingleValueMapper<u64>;

    #[view(getStakeLimit)]
    #[storage_mapper("stake_limit")]
    fn stake_limit(&self) -> SingleValueMapper<u64>;

    #[view(getStakedNfts)]
    #[storage_mapper("staked_nfts")]
    fn staked_nfts(&self, staker: &ManagedAddress) -> UnorderedSetMapper<u64>;

    #[view(getStakingEnabled)]
    #[storage_mapper("staking_enabled")]
    fn staking_enabled(&self) -> SingleValueMapper<bool>;

    #[view(getAllowedStakers)]
    #[storage_mapper("allowed_stakers")]
    fn allowed_stakers(&self) -> SetMapper<ManagedAddress>;

    #[view(getNftCollection)]
    #[storage_mapper("nft_collection")]
    fn nft_collection(&self) -> SingleValueMapper<TokenIdentifier>;

    #[view(getRewardsToken)]
    #[storage_mapper("rewards_token")]
    fn rewards_token(&self) -> SingleValueMapper<EgldOrEsdtTokenIdentifier>;

    #[view(getTokensRewardPerEpoch)]
    #[storage_mapper("tokens_reward_per_epoch")]
    fn tokens_reward_per_epoch(&self) -> SingleValueMapper<BigUint>;

    #[view(getRewards)]
    #[storage_mapper("rewards")]
    fn rewards(&self, user: &ManagedAddress) -> SingleValueMapper<BigUint>;
    
    #[view(getRewardsComputationTimestamp)]
    #[storage_mapper("rewards_computation_epochs")]
    fn rewards_computation_epochs(&self, user: &ManagedAddress) -> SingleValueMapper<u64>;

    #[view(getRewardsSupply)]
    #[storage_mapper("rewardsSupply")]
    fn rewards_supply(&self) -> SingleValueMapper<BigUint>;

    #[view(getStakers)]
    #[storage_mapper("stakers")]
    fn stakers(&self) -> SetMapper<ManagedAddress>;
}
