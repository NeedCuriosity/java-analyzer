import java.util.List;

class Account {
    int balance;
    void close() {}
    void deposit(int amount) {}
}

class Main {
    Account account;

    void beforeCall(Account account, List<String> names) {
        account./*@call*/
        account.close();
    }

    int beforeReturn(Account account) {
        account./*@ret*/
        return account.balance;
    }

    void beforeDeclaration(Account account, List<String> names) {
        names./*@decl*/
        int size = names.size();
        String text = "a";
    }

    void beforeControlFlow(Account account, List<String> names) {
        account./*@ctrl*/
        if (names.isEmpty()) {
            account.close();
        }
        this.account./*@loop*/
        for (String name : names) {
            account.deposit(name.length());
        }
    }

    void beforeBrace(Account account) {
        account./*@brace*/
    }

    void withPrefix(Account account) {
        account.dep/*@prefix*/
        account.close();
    }
}
//? complete ret has close deposit balance
//? complete decl has size isEmpty add
//? complete ctrl has close deposit balance
//? complete loop has close deposit balance
//? complete brace has close deposit balance
