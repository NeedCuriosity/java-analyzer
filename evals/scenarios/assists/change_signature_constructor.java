//- /app/Money.java before
package app;

public class Money {
    private final long cents;

    public Money(long cents, String currency) {
        this.cents = cents;
    }

    public Money() {
        this(0, "EUR");
    }
}
//- /app/Money.java
package app;

public class Money {
    private final long cents;

    public /*@ctor*/Money(String currency, long amount) {
        this.cents = cents;
    }

    public Money() {
        this(0, "EUR");
    }
}
//- /app/Bonus.java
package app;

class Bonus extends Money {
    Bonus(long cents) {
        super(cents, "USD");
    }
}
//- /app/Main.java
package app;

class Main {
    Money price = new Money(250, "EUR");
    Money other = new Money();
}
//? assist change_signature ctor ; has "this.cents = amount;" ; has "this(\"EUR\", 0);" ; has "super(\"USD\", cents);" ; has "new Money(\"EUR\", 250);" ; has "Money other = new Money();"
