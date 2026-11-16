//- /lombok/AccessLevel.java
package lombok;
public enum AccessLevel { PUBLIC, MODULE, PROTECTED, PACKAGE, PRIVATE, NONE }
//- /lombok/ToString.java
package lombok;
public @interface ToString {}
//- /lombok/EqualsAndHashCode.java
package lombok;
public @interface EqualsAndHashCode { boolean callSuper() default false; }
//- /lombok/With.java
package lombok;
public @interface With { AccessLevel value() default AccessLevel.PUBLIC; }
//- /lombok/AllArgsConstructor.java
package lombok;
public @interface AllArgsConstructor { String staticName() default ""; AccessLevel access() default AccessLevel.PUBLIC; }
//- /lombok/Value.java
package lombok;
public @interface Value { String staticConstructor() default ""; }
//- /lombok/experimental/FieldNameConstants.java
package lombok.experimental;
public @interface FieldNameConstants {
    lombok.AccessLevel level() default lombok.AccessLevel.PUBLIC;
    boolean asEnum() default false;
    String innerTypeName() default "";
    boolean onlyExplicitlyIncluded() default false;
    @interface Exclude {}
    @interface Include {}
}
//- /lombok/experimental/StandardException.java
package lombok.experimental;
public @interface StandardException { lombok.AccessLevel access() default lombok.AccessLevel.PUBLIC; }
//- /app/Person.java
package app;

import lombok.EqualsAndHashCode;
import lombok.ToString;
import lombok.experimental.FieldNameConstants;

@ToString
@EqualsAndHashCode
@FieldNameConstants
public class Person {
    private String /*@name_field*/name;
    private int age;
    @FieldNameConstants.Exclude
    private int internal;
}
//- /app/Range.java
package app;

import lombok.AllArgsConstructor;
import lombok.With;

@With
@AllArgsConstructor
public class Range {
    private final int low;
    private final int high;
    private final int step = 1;
}
//- /app/Temperature.java
package app;

import lombok.Value;

@Value(staticConstructor = "of")
public class Temperature {
    double degrees;
    String unit;
}
//- /app/ParseException.java
package app;

import lombok.experimental.StandardException;

@StandardException
public class ParseException extends Exception {
}
//- /app/Settings.java
package app;

import lombok.AllArgsConstructor;

@AllArgsConstructor
public class Settings {
    private int retries = 3;

    Settings() {
    }

    int retries() {
        return retries;
    }
}
//- /app/Main.java
package app;

class Main {
    String run(Person person, Person other) throws ParseException {
        String key = Person.Fields./*@name_const*/name;
        switch (key) {
            case Person.Fields.age:
                break;
            default:
                break;
        }
        boolean same = person.equals(other) && person.canEqual(other) && person.hashCode() == 0;
        int retries = new Settings(5).retries() + new Settings().retries();
        Range range = new Range(1, 5).withLow(2).withHigh(3);
        Temperature t = Temperature.of(20.5, "C");
        if (!same) {
            throw new ParseException("bad", new RuntimeException());
        }
        Exception e = new ParseException(new IllegalStateException());
        return key + retries + person.toString() + range + t.getDegrees() + t.getUnit() + e + Person.Fields./*@fields*/age;
    }
}
//? diag none
//? def name_const -> name_field
//? complete fields has name age
//? complete fields lacks internal
//? diag only field-may-be-final
