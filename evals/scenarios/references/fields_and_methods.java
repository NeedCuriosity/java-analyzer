//- /app/Account.java
package app;

public class Account {
    private long /*@balance*/balance;

    public long /*@get*/getBalance() { return /*@r1*/balance; }

    public void /*@deposit*/deposit(long amount) {
        /*@w1*/balance += amount;
        this./*@w2*/balance = Math.max(0, /*@r2*/balance);
    }
}
//- /app/Bank.java
package app;

class Bank {
    long total(Account a, Account b) {
        a./*@d1*/deposit(1);
        b./*@d2*/deposit(2);
        return a./*@g1*/getBalance() + b./*@g2*/getBalance();
    }
}
//? refs balance = r1 w1 w2 r2
//? refs deposit = d1 d2
//? refs get = g1 g2
//? refs g1 = g1 g2
//? diag none
