//- /lombok/AccessLevel.java
package lombok;
public enum AccessLevel { PUBLIC, MODULE, PROTECTED, PACKAGE, PRIVATE, NONE }
//- /lombok/Builder.java
package lombok;
public @interface Builder {
    String builderMethodName() default "builder";
    String buildMethodName() default "build";
    String builderClassName() default "";
    boolean toBuilder() default false;
    AccessLevel access() default AccessLevel.PUBLIC;
    String setterPrefix() default "";
    @interface Default {}
}
//- /lombok/Singular.java
package lombok;
public @interface Singular { String value() default ""; boolean ignoreNullCollections() default false; }
//- /lombok/Getter.java
package lombok;
public @interface Getter { AccessLevel value() default AccessLevel.PUBLIC; boolean lazy() default false; }
//- /app/Order.java
package app;

import java.util.List;
import java.util.Map;
import lombok.Builder;
import lombok.Getter;
import lombok.Singular;

@Getter
@Builder(builderMethodName = "make", buildMethodName = "create", toBuilder = true, setterPrefix = "with")
public class Order {
    private final String /*@id*/id;
    @Singular
    private final List<String> /*@items*/items;
    @Singular("tag")
    private final Map<String, Integer> tags;
    @Builder.Default
    private final int quantity = 1;
    @Getter(lazy = true)
    private final String summary = "summary";
}
//- /app/Money.java
package app;

import lombok.Builder;

public class Money {
    private final long cents;

    private Money(long cents) {
        this.cents = cents;
    }

    @Builder(builderMethodName = "of")
    static Money create(long amount, String currency) {
        return new Money(amount);
    }

    long cents() {
        return cents;
    }
}
//- /app/Main.java
package app;

import java.util.List;

class Main {
    Order run() {
        Order order = Order.make()
                ./*@with_id*/withId("1")
                .withItem("a")
                .withItems(List.of("b", "c"))
                .clearItems()
                .withTag("k", 1)
                .withQuantity(3)
                ./*@create*/create();
        Order copy = order.toBuilder().withItem("d").create();
        Money money = Money.of().amount(3).currency("EUR").build();
        int n = copy.getQuantity() + copy.getItems().size() + copy.getSummary().length();
        return n > money.cents() ? order : copy;
    }

    void complete(Order.OrderBuilder builder) {
        builder./*@members*/hashCode();
    }
}
//? diag none
//? def with_id -> id
//? hover create ~ "Order create()"
//? complete members has withId withItem withItems clearItems withTag withTags clearTags withQuantity create
//? complete members lacks build withSummary id items
