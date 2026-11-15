import java.util.List;

class Account {
    private int secret;
    int visible;
    static int COUNT;
    void deposit(int amount) {}
    static Account open() { return null; }
}

class Main {
    private int own;

    void run(Account account, List<String> names, String text) {
        account./*@inst*/;
        Account./*@stat*/;
        names.get(0)./*@chain*/;
        this./*@self*/;
        text.sub/*@prefix*/;
        names.stream().map(s -> s./*@lambda*/);
    }
}
//? complete inst has deposit visible hashCode
//? complete inst lacks secret
//? complete stat has open COUNT
//? complete stat lacks deposit visible
//? complete chain has length isEmpty charAt
//? complete self has own run
//? complete prefix has substring subSequence
//? complete prefix lacks length
//? complete prefix first substring
//? complete lambda has length trim
