class Account {
    int balance;
    void close() {}
    void deposit(int amount) {}
    static Account open(String owner) { return null; }
}

class Main {
    void run(Account account) throws InterruptedException {
        account./*@member*/;
        account.dep/*@prefix*/;
        Account./*@stat*/;
        account./*@called*/(1);
        new /*@ctor*/;
    }
}
//? insert member close ~ "close()"
//? insert member deposit ~ "deposit($0)" snippet
//? insert member hashCode ~ "hashCode()"
//? insert member wait ~ "wait("
//? insert member equals ~ "equals($0)" snippet
//? insert member balance ~ "balance" plain
//? insert prefix deposit ~ "deposit($0)" snippet
//? insert stat open ~ "open($0)" snippet
//? insert called deposit ~ "deposit" plain
//? insert ctor Account ~ "Account("
