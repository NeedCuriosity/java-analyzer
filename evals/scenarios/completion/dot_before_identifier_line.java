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
//? complete call has close deposit balance
//? insert call close ~ "close()"
//? insert call deposit ~ "deposit($0)" snippet
//? complete prefix has deposit
//? complete prefix lacks close
//? insert prefix deposit ~ "deposit($0)" snippet
